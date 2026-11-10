import test from 'node:test';
import assert from 'node:assert/strict';
import { main, STAGES } from './verify_all.mjs';

function captureConsole(run) {
  const out = [];
  const err = [];
  const log = console.log;
  const error = console.error;
  console.log = (...parts) => out.push(parts.join(' '));
  console.error = (...parts) => err.push(parts.join(' '));
  try {
    return { code: run(), out, err };
  } finally {
    console.log = log;
    console.error = error;
  }
}

function recordingRun(failMap = {}) {
  const calls = [];
  const run = (cmd, env) => {
    calls.push({ cmd, env });
    const marker = Object.keys(failMap).find((key) => cmd.includes(key));
    return marker ? { status: 1, output: failMap[marker] } : { status: 0, output: 'ok' };
  };
  return { calls, run };
}

test('main: full green run exits 0, runs every stage with CARGO_BUILD_JOBS=4, prints GO verdict', () => {
  const { calls, run } = recordingRun();
  const { code, out } = captureConsole(() => main([], { run, resolveBase: () => 'HEAD' }));
  assert.equal(code, 0);
  assert.equal(calls.length, STAGES.length);
  assert.ok(calls.every((call) => call.env.CARGO_BUILD_JOBS === '4'));
  const passLines = out.filter((line) => line.startsWith('PASS  '));
  assert.equal(passLines.length, STAGES.length);
  const verdict = out.find((line) => line.startsWith('VERIFY-ALL:'));
  assert.match(
    verdict,
    new RegExp(`^VERIFY-ALL: GO \\(${STAGES.length} stage\\(s\\), \\d+s\\)$`),
  );
});

test('main: failed check fails the run, skips clippy and nextest unexecuted, prints NO-GO', () => {
  const { calls, run } = recordingRun({ check: 'error: check failed' });
  const { code, out } = captureConsole(() => main([], { run, resolveBase: () => 'HEAD' }));
  assert.equal(code, 1);
  assert.ok(calls.some((call) => call.cmd.includes('check')));
  assert.ok(calls.every((call) => !call.cmd.includes('clippy')));
  assert.ok(calls.every((call) => !call.cmd.includes('nextest')));
  assert.equal(calls.length, 9);
  assert.ok(out.includes('SKIP  clippy (requires check)'));
  assert.ok(out.includes('SKIP  nextest (requires check)'));
  assert.ok(out.includes('VERIFY-ALL: NO-GO (failed: check; skipped: clippy, nextest)'));
});

test('main: explicit stage subset runs exactly the named stage with requires ignored', () => {
  const { calls, run } = recordingRun();
  const { code, out } = captureConsole(() => main(['clippy'], { run, resolveBase: () => 'HEAD' }));
  assert.equal(code, 0);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].cmd, ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings']);
  assert.ok(out.includes('VERIFY-ALL: GO (1 stage(s), 0s)'));
});

test('main: unknown stage name exits 2 without executing and lists known stage names on stderr', () => {
  const { calls, run } = recordingRun();
  const { code, err } = captureConsole(() =>
    main(['fmt', 'nope'], { run, resolveBase: () => 'HEAD' }),
  );
  assert.equal(code, 2);
  assert.equal(calls.length, 0);
  const message = err.join('\n');
  assert.ok(message.includes('nope'));
  for (const stage of STAGES) assert.ok(message.includes(stage.name));
});

test('STAGES: 11 unique stages, fmt/check/clippy open, nextest closes, requires all resolve', () => {
  assert.equal(STAGES.length, 11);
  const names = STAGES.map((stage) => stage.name);
  assert.equal(new Set(names).size, names.length);
  assert.deepEqual(names.slice(0, 3), ['fmt', 'check', 'clippy']);
  assert.equal(names[names.length - 1], 'nextest');
  for (const stage of STAGES) {
    for (const req of stage.requires) {
      assert.ok(names.includes(req), `${stage.name} requires unknown stage ${req}`);
    }
  }
  assert.deepEqual(STAGES.find((stage) => stage.name === 'clippy').requires, ['check']);
  assert.deepEqual(STAGES.find((stage) => stage.name === 'nextest').requires, ['check']);
});

test('main: injected resolveBase lands in the tests-untouched command', () => {
  const { calls, run } = recordingRun();
  const { code } = captureConsole(() =>
    main(['tests-untouched'], { run, resolveBase: () => 'abc123' }),
  );
  assert.equal(code, 0);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].cmd.slice(-3), [
    'node',
    'scripts/quality/assert_tests_untouched.mjs',
    'abc123',
  ]);
});

test('main: failure prints only the last 30 non-empty output lines under a tail header', () => {
  const lines = Array.from({ length: 40 }, (_, i) => `noise line ${i + 1}`);
  const { run } = recordingRun({ fmt: lines.join('\n') });
  const { code, out, err } = captureConsole(() =>
    main(['fmt'], { run, resolveBase: () => 'HEAD' }),
  );
  assert.equal(code, 1);
  const headerIndex = err.indexOf('--- fmt output (tail) ---');
  assert.notEqual(headerIndex, -1);
  const tail = err[headerIndex + 1].split('\n');
  assert.equal(tail.length, 30);
  assert.equal(tail[0], 'noise line 11');
  assert.equal(tail[tail.length - 1], 'noise line 40');
  assert.ok(out.includes('FAIL  0s  fmt'));
  assert.ok(out.includes('VERIFY-ALL: NO-GO (failed: fmt)'));
});
