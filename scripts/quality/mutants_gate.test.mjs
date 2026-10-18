// Unit + integration tests for scripts/quality/mutants_gate.mjs
// Contract source: approved plan Phase 0 item 5 (Issue #214, T4) —
// gate decision is "zero NEW surviving mutants vs baseline" on the whole
// workspace. Baseline is snapshotted to target/quality/mutants-baseline.json.
// Run: node --test scripts/quality/mutants_gate.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { parseOutcomes, compareSurvivors, main } from './mutants_gate.mjs';

function outcome(name, summary, pkg = 'anaden-strategies', file = 'crates/anaden-strategies/src/catalog.rs') {
  return {
    scenario: { Mutant: { name, package: pkg, file, function: { function_name: 'f' } } },
    summary,
    log_path: 'log/x.log',
    diff_path: 'diff/x.diff',
    phase_results: [],
  };
}

const SAMPLE = {
  total_mutants: 5,
  missed: 2,
  caught: 2,
  timeout: 1,
  unviable: 0,
  success: true,
  start_time: '2026-10-17T00:00:00Z',
  end_time: '2026-10-17T00:05:00Z',
  cargo_mutants_version: '27.0.0',
  outcomes: [
    { scenario: 'Baseline', summary: 'Success', phase_results: [] },
    outcome('a:1:1: replace x with 1', 'CaughtMutant'),
    outcome('b:2:1: replace x with 2', 'MissedMutant'),
    outcome('c:3:1: replace x with 3', 'Timeout'),
    outcome('d:4:1: replace x with 4', 'Unviable'),
    outcome('e:5:1: replace x with 5', 'MissedMutant'),
  ],
};

// ---------------------------------------------------------------------------
// parseOutcomes
// ---------------------------------------------------------------------------

test('parseOutcomes: survivors = MissedMutant + TimeoutMutant, keyed by mutant name', () => {
  const parsed = parseOutcomes(SAMPLE);
  assert.deepEqual(parsed.survivors.map((s) => s.name).sort(), [
    'b:2:1: replace x with 2',
    'c:3:1: replace x with 3',
    'e:5:1: replace x with 5',
  ]);
  assert.equal(parsed.totals.totalMutants, 5);
  assert.equal(parsed.meta.cargo_mutants_version, '27.0.0');
});

test('parseOutcomes: a run with survivors is usable (success flag is cargo-mutants exit semantics, not integrity)', () => {
  // The calibration reality: success=false simply because 3 mutants survived.
  const parsed = parseOutcomes({ ...SAMPLE, success: false });
  assert.equal(parsed.survivors.length, 3);
});

test('parseOutcomes: inconsistent totals are unusable (throws — fail-closed)', () => {
  assert.throws(
    () => parseOutcomes({ ...SAMPLE, total_mutants: 99 }),
    /inconsistent/,
  );
  assert.throws(
    () => parseOutcomes({ ...SAMPLE, missed: 5 }),
    /inconsistent/,
  );
});

// ---------------------------------------------------------------------------
// compareSurvivors
// ---------------------------------------------------------------------------

test('compareSurvivors: detects new and fixed survivors by name', () => {
  const baseline = [{ name: 'b:2:1: replace x with 2' }, { name: 'e:5:1: replace x with 5' }];
  const current = [{ name: 'b:2:1: replace x with 2' }, { name: 'z:9:1: replace y with 0' }];
  const diff = compareSurvivors(baseline, current);
  assert.deepEqual(diff.newSurvivors.map((s) => s.name), ['z:9:1: replace y with 0']);
  assert.deepEqual(diff.fixedSurvivors.map((s) => s.name), ['e:5:1: replace x with 5']);
});

test('compareSurvivors: identical sets -> no new survivors', () => {
  const a = [{ name: 'n1' }, { name: 'n2' }];
  const diff = compareSurvivors(a, [{ name: 'n2' }, { name: 'n1' }]);
  assert.deepEqual(diff.newSurvivors, []);
  assert.deepEqual(diff.fixedSurvivors, []);
});

// ---------------------------------------------------------------------------
// CLI: snapshot + check (tmpdir integration)
// ---------------------------------------------------------------------------

function makeTmpRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'mut-gate-'));
}

function writeOutcomes(root, json) {
  fs.mkdirSync(path.join(root, 'target', 'mutants.out'), { recursive: true });
  fs.writeFileSync(path.join(root, 'target', 'mutants.out', 'outcomes.json'), JSON.stringify(json));
}

test('main snapshot: writes mutants-baseline.json with survivors and totals', () => {
  const root = makeTmpRoot();
  try {
    writeOutcomes(root, SAMPLE);
    const code = main(['snapshot'], { root });
    assert.equal(code, 0);
    const baseline = JSON.parse(
      fs.readFileSync(path.join(root, 'target', 'quality', 'mutants-baseline.json'), 'utf8'),
    );
    assert.equal(baseline.survivors.length, 3);
    assert.equal(baseline.totals.totalMutants, 5);
    assert.ok(baseline.generatedAt);
    assert.equal(baseline.sourceRun.cargo_mutants_version, '27.0.0');
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main check: no new survivors passes; new survivor fails with exit 1', () => {
  const root = makeTmpRoot();
  try {
    writeOutcomes(root, SAMPLE);
    assert.equal(main(['snapshot'], { root }), 0);
    // rerun with the same survivor set -> pass
    assert.equal(main(['check'], { root }), 0);
    // rerun with an extra survivor -> fail (keep totals consistent, as the
    // real tool would)
    const worse = JSON.parse(JSON.stringify(SAMPLE));
    worse.outcomes.push(outcome('z:9:1: replace y with 0', 'MissedMutant'));
    worse.total_mutants += 1;
    worse.missed += 1;
    writeOutcomes(root, worse);
    assert.equal(main(['check'], { root }), 1);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main check: missing baseline fails closed with exit 2', () => {
  const root = makeTmpRoot();
  try {
    writeOutcomes(root, SAMPLE);
    assert.equal(main(['check'], { root }), 2);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main check: missing current run fails closed with exit 2', () => {
  const root = makeTmpRoot();
  try {
    writeOutcomes(root, SAMPLE);
    assert.equal(main(['snapshot'], { root }), 0);
    fs.rmSync(path.join(root, 'target', 'mutants.out'), { recursive: true, force: true });
    assert.equal(main(['check'], { root }), 2);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
