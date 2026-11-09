import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

export function buildStages(base = 'HEAD') {
  return [
    { name: 'fmt', requires: [], cmd: ['cargo', 'fmt', '--all', '--check'] },
    { name: 'check', requires: [], cmd: ['cargo', 'check', '--all'] },
    {
      name: 'clippy',
      requires: ['check'],
      cmd: ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings'],
    },
    { name: 'node-tests', requires: [], cmd: ['node', 'scripts/quality/run_node_tests.mjs'] },
    { name: 'ast-grep-test', requires: [], cmd: ['ast-grep', 'test', '--skip-snapshot-tests'] },
    { name: 'ast-grep-scan', requires: [], cmd: ['ast-grep', 'scan'] },
    { name: 'complexity', requires: [], cmd: ['node', 'scripts/quality/complexity_gate.mjs'] },
    { name: 'duplication', requires: [], cmd: ['node', 'scripts/quality/duplication_gate.mjs'] },
    { name: 'comment-gate', requires: [], cmd: ['node', 'scripts/quality/comment_gate.mjs'] },
    {
      name: 'tests-untouched',
      requires: [],
      cmd: ['node', 'scripts/quality/assert_tests_untouched.mjs', base],
    },
    { name: 'nextest', requires: ['check'], cmd: ['cargo', 'nextest', 'run', '--workspace'] },
  ];
}

export const STAGES = buildStages();

export function resolveMergeBase() {
  const result = spawnSync('git', ['merge-base', 'HEAD', 'origin/master'], {
    encoding: 'utf8',
    shell: process.platform === 'win32',
    stdio: 'pipe',
  });
  const base = String(result.stdout ?? '').trim();
  if (result.status === 0 && base !== '') return base;
  console.error(
    'verify-all: merge-base with origin/master failed; tests-untouched base falls back to HEAD',
  );
  return 'HEAD';
}

function spawnStage(cmd, env) {
  const result = spawnSync(cmd[0], cmd.slice(1), {
    cwd: repoRoot,
    shell: process.platform === 'win32',
    stdio: 'pipe',
    env,
    encoding: 'buffer',
    maxBuffer: 64 * 1024 * 1024,
  });
  const output = Buffer.concat([
    result.stdout ?? Buffer.alloc(0),
    result.stderr ?? Buffer.alloc(0),
  ]).toString('utf8');
  if (result.error) {
    const separator = output === '' ? '' : '\n';
    return { status: 1, output: `${output}${separator}${result.error.message}` };
  }
  return { status: result.status ?? 1, output };
}

function tailLines(output, max = 30) {
  return String(output ?? '')
    .split(/\r?\n/)
    .filter((line) => line.trim() !== '')
    .slice(-max);
}

function selectStages(stages, argv) {
  if (argv.length === 0) return { ok: true, gated: true, stages };
  const known = new Set(stages.map((stage) => stage.name));
  const unknown = argv.filter((name) => !known.has(name));
  if (unknown.length > 0) {
    return {
      ok: false,
      message: `verify-all: unknown stage(s): ${unknown.join(', ')}; known stages: ${stages
        .map((stage) => stage.name)
        .join(', ')}`,
    };
  }
  const requested = new Set(argv);
  return { ok: true, gated: false, stages: stages.filter((stage) => requested.has(stage.name)) };
}

export function main(argv = [], opts = {}) {
  const stages = opts.stages ?? buildStages((opts.resolveBase ?? resolveMergeBase)());
  const run = opts.run ?? spawnStage;
  const selection = selectStages(stages, argv);
  if (!selection.ok) {
    console.error(selection.message);
    return 2;
  }
  const env = { ...process.env, CARGO_BUILD_JOBS: '4' };
  const outcomes = new Map();
  const failed = [];
  const skipped = [];
  const runStarted = performance.now();
  for (const stage of selection.stages) {
    const blockedBy = selection.gated
      ? stage.requires.find((name) => outcomes.get(name) !== 'pass')
      : undefined;
    if (blockedBy !== undefined) {
      outcomes.set(stage.name, 'skip');
      skipped.push(stage.name);
      console.log(`SKIP  ${stage.name} (requires ${blockedBy})`);
      continue;
    }
    const stageStarted = performance.now();
    const result = run(stage.cmd, env);
    const seconds = Math.round((performance.now() - stageStarted) / 1000);
    const passed = result.status === 0;
    outcomes.set(stage.name, passed ? 'pass' : 'fail');
    console.log(`${passed ? 'PASS' : 'FAIL'}  ${seconds}s  ${stage.name}`);
    if (passed) continue;
    failed.push(stage.name);
    console.error(`--- ${stage.name} output (tail) ---`);
    console.error(tailLines(result.output).join('\n'));
  }
  const totalSeconds = Math.round((performance.now() - runStarted) / 1000);
  if (failed.length > 0) {
    const skipClause = skipped.length > 0 ? `; skipped: ${skipped.join(', ')}` : '';
    console.log(`VERIFY-ALL: NO-GO (failed: ${failed.join(', ')}${skipClause})`);
    return 1;
  }
  console.log(`VERIFY-ALL: GO (${selection.stages.length} stage(s), ${totalSeconds}s)`);
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exitCode = main(process.argv.slice(2));
}
