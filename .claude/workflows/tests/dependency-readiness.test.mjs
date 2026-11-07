import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = 'const taskDeclaredDeps';
const END = "phase('Implement');";

const loadApi = () => {
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END, begin);
  assert.ok(begin >= 0, 'dependency-readiness block start not found');
  assert.ok(end > begin, 'dependency-readiness block end not found');
  const block = src.slice(begin, end);
  return new Function(
    `${block}; return { taskDeclaredDeps, evaluateDependencyReadiness, buildDependencyReadinessRationale };`
  )();
};

test('dependency-readiness pure block evaluates', () => {
  const api = loadApi();
  assert.equal(typeof api.taskDeclaredDeps, 'function');
  assert.equal(typeof api.evaluateDependencyReadiness, 'function');
  assert.equal(typeof api.buildDependencyReadinessRationale, 'function');
});

test('extracts declared dependency ids as string array', () => {
  const deps = loadApi().taskDeclaredDeps({ id: 'T2', dependencies: ['T1', 'T0'] });
  assert.deepEqual(deps, ['T1', 'T0']);
});

test('non-array / null / missing dependencies → [] (no deps, trivially ready)', () => {
  const api = loadApi();
  for (const task of [
    { id: 'T1' },
    { id: 'T1', dependencies: null },
    { id: 'T1', dependencies: undefined },
    { id: 'T1', dependencies: 'T0' },
    { id: 'T1', dependencies: 42 },
    { id: 'T1', dependencies: { 0: 'T0' } },
    null,
    'x',
    7,
  ]) {
    assert.deepEqual(api.taskDeclaredDeps(task), [], JSON.stringify(task));
  }
});

test('filters non-string and empty entries from dependencies array', () => {
  const deps = loadApi().taskDeclaredDeps({
    id: 'T3',
    dependencies: ['T1', 2, null, undefined, '', 'T2'],
  });
  assert.deepEqual(deps, ['T1', 'T2']);
});

const READY_RECORD = {
  committed: true,
  reachable: true,
  greenEvidence: true,
  commitSha: 'abc1234',
};
const READY_RECORD_2 = {
  committed: true,
  reachable: true,
  greenEvidence: true,
  commitSha: 'def5678',
};

test('UC-1: all deps committed + reachable + green → ready (dispatchable)', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    { T1: READY_RECORD },
  );
  assert.equal(r.ready, true, JSON.stringify(r));
  assert.equal(r.verdict, 'ready');
  assert.equal(r.reason, null);
  assert.equal(r.taskId, 'T2');
  assert.deepEqual(
    r.deps.map((d) => d.verdict),
    ['ready'],
  );
});

test('UC-1: multiple deps all ready → ready; per-dep verdicts preserved in order', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T3', dependencies: ['T1', 'T2'] },
    { T1: READY_RECORD, T2: READY_RECORD_2 },
  );
  assert.equal(r.ready, true);
  assert.deepEqual(
    r.deps.map((d) => ({ dep: d.dep, verdict: d.verdict })),
    [
      { dep: 'T1', verdict: 'ready' },
      { dep: 'T2', verdict: 'ready' },
    ],
  );
});

test('UC-3: deps-empty task → trivially ready even with null depRecords', () => {
  const api = loadApi();
  for (const depRecords of [null, undefined, {}, 'garbage']) {
    for (const task of [{ id: 'T1', dependencies: [] }, { id: 'T1' }]) {
      const r = api.evaluateDependencyReadiness(task, depRecords);
      assert.equal(r.ready, true, JSON.stringify({ task, depRecords, r }));
      assert.equal(r.verdict, 'ready');
      assert.deepEqual(r.deps, []);
    }
  }
});

test('malformed task input (null / string / number) → trivially ready, taskId null', () => {
  const api = loadApi();
  for (const task of [null, 'T1', 42]) {
    const r = api.evaluateDependencyReadiness(task, {});
    assert.equal(r.ready, true, JSON.stringify(task));
    assert.equal(r.verdict, 'ready');
    assert.equal(r.taskId, null);
    assert.deepEqual(r.deps, []);
  }
});

test('UC-2: dep committed=false (working-tree only) → blocked dep-not-committed', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    { T1: { ...READY_RECORD, committed: false, commitSha: null } },
  );
  assert.equal(r.ready, false, JSON.stringify(r));
  assert.equal(r.verdict, 'dep-not-committed');
  assert.equal(r.deps[0].verdict, 'dep-not-committed');
  assert.ok(typeof r.reason === 'string' && r.reason.includes('T1'));
});

test('UC-2 variant: committed but reachable=false (stranded snapshot) → blocked dep-not-committed', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    { T1: { ...READY_RECORD, reachable: false } },
  );
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'dep-not-committed');
  assert.equal(r.deps[0].verdict, 'dep-not-committed');
});

test('UC-2: one ready dep + one working-tree-only dep → blocked (fail-closed aggregation)', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T3', dependencies: ['T1', 'T2'] },
    { T1: READY_RECORD, T2: { ...READY_RECORD, committed: false } },
  );
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'dep-not-committed');
  assert.deepEqual(
    r.deps.map((d) => d.verdict),
    ['ready', 'dep-not-committed'],
  );
});

test('UC-3: dep record absent from collector output → unknown-evidence, blocked (never green-by-default)', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    {},
  );
  assert.equal(r.ready, false, JSON.stringify(r));
  assert.equal(r.verdict, 'unknown-evidence');
  assert.equal(r.deps[0].verdict, 'unknown-evidence');
  assert.equal(r.deps[0].committed, null);
  assert.equal(r.deps[0].reachable, null);
  assert.equal(r.deps[0].greenEvidence, null);
});

test('UC-3: malformed dep record (null / string / array) → unknown-evidence, blocked', () => {
  const api = loadApi();
  for (const bad of [null, 'green', 42, ['committed']]) {
    const r = api.evaluateDependencyReadiness(
      { id: 'T2', dependencies: ['T1'] },
      { T1: bad },
    );
    assert.equal(r.ready, false, JSON.stringify(bad));
    assert.equal(r.verdict, 'unknown-evidence');
    assert.equal(r.deps[0].verdict, 'unknown-evidence');
  }
});

test('UC-3: non-boolean evidence flags → unknown-evidence (strict typing, fail-closed)', () => {
  const api = loadApi();
  for (const flags of [
    { committed: 'yes', reachable: true, greenEvidence: true },
    { committed: 1, reachable: true, greenEvidence: true },
    { committed: true, reachable: undefined, greenEvidence: true },
    { committed: true, reachable: true, greenEvidence: 'true' },
    { reachable: true, greenEvidence: true },
    { committed: true, greenEvidence: true },
    { committed: true, reachable: true },
    {},
  ]) {
    const r = api.evaluateDependencyReadiness(
      { id: 'T2', dependencies: ['T1'] },
      { T1: flags },
    );
    assert.equal(r.ready, false, JSON.stringify(flags));
    assert.equal(r.deps[0].verdict, 'unknown-evidence', JSON.stringify(flags));
  }
});

test('UC-3: committed + reachable but greenEvidence=false → dep-evidence-missing, blocked', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    { T1: { ...READY_RECORD, greenEvidence: false } },
  );
  assert.equal(r.ready, false, JSON.stringify(r));
  assert.equal(r.verdict, 'dep-evidence-missing');
  assert.equal(r.deps[0].verdict, 'dep-evidence-missing');
});

test('committed=false + greenEvidence=true → still blocked (all three required, no partial credit)', () => {
  const r = loadApi().evaluateDependencyReadiness(
    { id: 'T2', dependencies: ['T1'] },
    { T1: { ...READY_RECORD, committed: false } },
  );
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'dep-not-committed');
});

test('null / non-object depRecords with declared deps → all unknown-evidence, blocked', () => {
  const api = loadApi();
  for (const depRecords of [null, undefined, 'x', 9]) {
    const r = api.evaluateDependencyReadiness(
      { id: 'T2', dependencies: ['T1'] },
      depRecords,
    );
    assert.equal(r.ready, false, JSON.stringify(depRecords));
    assert.equal(r.verdict, 'unknown-evidence');
    assert.equal(r.deps[0].verdict, 'unknown-evidence');
  }
});

const UC1_TASKS = [
  { id: 'T1', files: ['a.rs'] },
  { id: 'T2', files: ['b.rs'], dependencies: ['T1'] },
];
const UC1_RECORDS = { T1: READY_RECORD };

test('rationale carries issue/runId/runTimestamp/recordedAt/treeHash/classifier and per-task verdicts', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    UC1_TASKS, UC1_RECORDS, 'deadbeef', 'run-x', '2026-09-04T00:00:00Z',
  );
  assert.equal(r.issue, 104);
  assert.equal(r.runId, 'run-x');
  assert.equal(r.runTimestamp, '2026-09-04T00:00:00Z');
  assert.equal(r.recordedAt, '2026-09-04T00:00:00Z');
  assert.equal(r.treeHash, 'deadbeef');
  assert.equal(typeof r.classifier, 'string');
  assert.ok(r.classifier.includes('evaluateDependencyReadiness'), r.classifier);
  const t2 = r.tasks.find((t) => t.taskId === 'T2');
  assert.equal(t2.ready, true);
  assert.deepEqual(
    t2.deps.map((d) => ({ dep: d.dep, verdict: d.verdict })),
    [{ dep: 'T1', verdict: 'ready' }],
  );
});

test('rationale UC-1: all deps ready → ready=true, dispatchable status', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    UC1_TASKS, UC1_RECORDS, 'deadbeef', 'run-x', 'ts',
  );
  assert.equal(r.ready, true);
  assert.equal(r.status, 'all-ready');
});

test('rationale UC-2: working-tree-only dep → ready=false, status dependency-not-ready', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    UC1_TASKS,
    { T1: { ...READY_RECORD, committed: false } },
    'deadbeef', 'run-x', 'ts',
  );
  assert.equal(r.ready, false);
  assert.equal(r.status, 'dependency-not-ready');
  const t2 = r.tasks.find((t) => t.taskId === 'T2');
  assert.equal(t2.verdict, 'dep-not-committed');
});

test('rationale UC-3: evidence log missing → ready=false fail-closed (never green-by-default)', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    UC1_TASKS, {}, 'deadbeef', 'run-x', 'ts',
  );
  assert.equal(r.ready, false);
  assert.equal(r.status, 'dependency-not-ready');
  const t2 = r.tasks.find((t) => t.taskId === 'T2');
  assert.equal(t2.verdict, 'unknown-evidence');
});

test('rationale: tasks with no declared deps anywhere → ready=true (trivially ready)', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    [
      { id: 'T1', files: ['a.rs'] },
      { id: 'T2', files: ['b.rs'] },
    ],
    {}, 'deadbeef', 'run-x', 'ts',
  );
  assert.equal(r.ready, true);
  assert.equal(r.status, 'all-ready');
  assert.ok(r.tasks.every((t) => t.deps.length === 0));
});

test('rationale: empty task list → ready=true', () => {
  const r = loadApi().buildDependencyReadinessRationale([], {}, 'h', 'run-x', 'ts');
  assert.equal(r.ready, true);
  assert.deepEqual(r.tasks, []);
});

test('rationale: malformed tasks input (non-array) → fail-closed blocked', () => {
  const api = loadApi();
  for (const bad of [null, undefined, 'x', 42, {}]) {
    const r = api.buildDependencyReadinessRationale(bad, {}, 'h', 'run-x', 'ts');
    assert.equal(r.ready, false, JSON.stringify(bad));
    assert.equal(r.status, 'dependency-not-ready');
    assert.deepEqual(r.tasks, []);
  }
});

test('rationale: empty/missing treeHash and runTimestamp → unknown (Issue #97 convention)', () => {
  const api = loadApi();
  let r = api.buildDependencyReadinessRationale(UC1_TASKS, UC1_RECORDS, '', 'run-x', '');
  assert.equal(r.treeHash, 'unknown');
  assert.equal(r.recordedAt, 'unknown');
  r = api.buildDependencyReadinessRationale(UC1_TASKS, UC1_RECORDS, 42, 'run-x', null);
  assert.equal(r.treeHash, 'unknown');
  assert.equal(r.recordedAt, 'unknown');
});

test('rationale: malformed task entries excluded from tasks array (taskId unresolvable)', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    [null, 'junk', { id: 'T1', files: ['a.rs'] }],
    {}, 'h', 'run-x', 'ts',
  );
  assert.deepEqual(r.tasks.map((t) => t.taskId), ['T1']);
});

test('rationale: mixed ready/blocked tasks aggregate fail-closed', () => {
  const r = loadApi().buildDependencyReadinessRationale(
    [
      { id: 'T1', files: ['a.rs'] },
      { id: 'T2', files: ['b.rs'], dependencies: ['T1'] },
      { id: 'T3', files: ['c.rs'], dependencies: ['T2'] },
    ],
    { T1: READY_RECORD },
    'h', 'run-x', 'ts',
  );
  assert.equal(r.ready, false);
  assert.equal(r.status, 'dependency-not-ready');
  const t3 = r.tasks.find((t) => t.taskId === 'T3');
  assert.equal(t3.verdict, 'unknown-evidence');
});

const WIRING_BEGIN = 'const _declaredDeps = taskDeclaredDeps(task);';
const WIRING_END = 'buildEngineerPrompt(laneResult, task, ticket, approval)';

const wiringBlock = () => {
  const begin = src.indexOf(WIRING_BEGIN);
  const end = src.indexOf(WIRING_END, begin);
  assert.ok(begin >= 0, 'dependency wiring block start not found');
  assert.ok(end > begin, 'dependency wiring block end not found');
  return src.slice(begin, end);
};

const loadWiringApi = () => {
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END, begin);
  assert.ok(begin >= 0 && end > begin, 'pure block present');
  const block = src.slice(begin, end);
  return new Function(
    `${block}; return { normalizeDependencyCollectorOutput, DEPENDENCY_COLLECTOR_SCHEMA };`
  )();
};

test('wiring: block nested inside ownership-serialization wiring region', () => {
  const wBegin = src.indexOf(WIRING_BEGIN);
  const wEnd = src.indexOf(WIRING_END, wBegin);
  const oBegin = src.indexOf("phase('Implement');");
  const oEnd = src.indexOf('const _slug', oBegin);
  assert.ok(oBegin > 0 && oEnd > oBegin, 'ownership wiring region present');
  assert.ok(wBegin > oBegin && wEnd < oEnd,
    'dependency wiring nested inside ownership wiring (runImplementTask region)');
});

test('wiring: sits after lane-gate fail-closed check, before engineer agent dispatch', () => {
  const wBegin = src.indexOf(WIRING_BEGIN);
  const laneMissing = src.indexOf("status: 'lane-missing'");
  const humanGated = src.indexOf("status: 'human-gated'");
  const dispatch = src.indexOf('buildEngineerPrompt(laneResult, task, ticket, approval)');
  assert.ok(laneMissing > 0, "lane-gate 'lane-missing' status present");
  assert.ok(humanGated > 0, "operator-gated 'human-gated' status present");
  assert.ok(dispatch > 0, 'engineer dispatch call present');
  assert.ok(laneMissing < wBegin, 'lane-gate check precedes dependency wiring');
  assert.ok(dispatch > wBegin, 'engineer dispatch follows dependency wiring');
});

test('wiring: collector agent dispatched once per gated task (sonnet + StructuredOutput schema)', () => {
  const w = wiringBlock();
  assert.match(w, /taskDeclaredDeps\(task\)/, 'declared deps resolved from task');
  assert.match(w, /\.length > 0/, 'collector gated on non-empty deps (deps-empty → straight dispatch)');
  assert.match(w, /model: 'sonnet'/, 'collector uses sonnet');
  assert.match(w, /schema: DEPENDENCY_COLLECTOR_SCHEMA/, 'StructuredOutput schema attached');
  assert.match(w, /git log -1 --format=%H/, 'last-commit SHA per dep files');
  assert.match(w, /git branch -a --contains/, 'reachability check (branch-contains)');
  assert.match(w, /\.omc\/logs\/\$\{runId\}\//, 'current-run evidence scan');
  assert.match(w, /run-metadata\.json/, 'prior-run metadata lookup (Issue #97)');
  assert.match(w, /greenEvidence/, 'green-evidence contract field');
  assert.match(w, /taskDeclaredFiles\(/, 'dep declared files sourced from estimate tasks');
});

test('wiring: gate call on collector records; unparseable output is fail-closed, never bypassed', () => {
  const w = wiringBlock();
  assert.match(w, /normalizeDependencyCollectorOutput\(/, 'collector output normalized');
  assert.match(w, /evaluateDependencyReadiness\(task,/, 'gate evaluated on normalized records');
  assert.match(w, /if \(!readiness\.ready\) \{/, 'blocked branch keyed on !readiness.ready');
  const bypass = w.match(/ready\s*=\s*true/);
  assert.equal(bypass, null, 'no hardcoded ready=true bypass of the gate');
});

test('wiring: blocked → persist rationale + status dependency-not-ready WITHOUT dispatch', () => {
  const w = wiringBlock();
  const blocked = w.lastIndexOf("'dependency-not-ready'");
  assert.ok(blocked > 0, "status 'dependency-not-ready' literal surfaced in wiring");
  assert.match(w, /dependency-readiness\.json/, 'persister file name dependency-readiness.json');
  assert.match(w, /EVIDENCE PERSISTER/, 'persister prompt shape (same as ownership-serialization persister)');
  assert.match(w, /model: 'haiku'/, 'persister uses haiku');
  assert.match(w, /buildDependencyReadinessRationale\(/, 'rationale payload built');
  assert.match(w, /precheckScope && precheckScope\.treeHash/, 'treeHash sourced from precheck scope');
  const ret = w.slice(Math.max(0, blocked - 500), blocked + 300);
  assert.ok(ret.includes('return {'), 'blocked branch returns per-task non-fatal status object');
  assert.match(w, /readiness\.reason/, 'reason surfaced in blocked return/log');
});

test('wiring: blocked return precedes wiring end (no dispatch on the blocked path)', () => {
  const w = wiringBlock();
  const blocked = w.indexOf("'dependency-not-ready'");
  const afterBlocked = w.slice(blocked);
  assert.equal(/buildEngineerPrompt/.test(afterBlocked), false,
    'wiring block must not contain the engineer dispatch (blocked path returns early)');
});

test('wiring: ready or deps-empty → dispatch unchanged (engineer agent call preserved)', () => {
  const wBegin = src.indexOf(WIRING_BEGIN);
  const dispatch = src.indexOf('buildEngineerPrompt(laneResult, task, ticket, approval)', wBegin);
  assert.ok(dispatch > wBegin, 'engineer dispatch follows the wiring block');
  const dispatchCtx = src.slice(dispatch - 60, dispatch + 220);
  assert.match(dispatchCtx, /model: 'sonnet'/, 'engineer dispatch keeps sonnet');
  assert.match(dispatchCtx, /engineer:\$\{task\.id\}/, 'engineer label unchanged');
});

test('no regression: operator-gated skip and lane-gate fail-closed precede dependency wiring', () => {
  const human = src.indexOf("status: 'human-gated'");
  const lane = src.indexOf("status: 'lane-missing'");
  const wBegin = src.indexOf(WIRING_BEGIN);
  assert.ok(human > 0 && human < wBegin, 'operator-gated skip still first');
  assert.ok(lane > human && lane < wBegin, 'lane-gate check still before dependency wiring');
});

test('no regression: ownership-serialization dispatch ordering intact (groups loop drives runImplementTask)', () => {
  const oBegin = src.indexOf("phase('Implement');");
  const oEnd = src.indexOf('const _slug', oBegin);
  const region = src.slice(oBegin, oEnd);
  assert.match(region, /ownershipPlan\.groups/, 'groups loop present');
  assert.match(region, /runImplementTask\(task\)/, 'dispatch loop still calls runImplementTask');
  assert.match(region, /resolveDispatchGroups/, 'group composition unchanged');
  assert.match(region, /'ownership-undeclared'/, 'ownership fail-closed status intact');
});

test('drift-guard: pure block stays runtime-free (no wiring smuggled into the pure block)', () => {
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END);
  const pure = src.slice(begin, end);
  assert.equal(/await\s/.test(pure), false, 'no await in pure block');
  assert.equal(pure.includes('agent('), false, 'no agent() calls in pure block');
  assert.equal(pure.includes('EVIDENCE PERSISTER'), false, 'no persister prompt in pure block');
});

test('drift-guard: wiring block does not redefine pure-block functions', () => {
  const w = wiringBlock();
  for (const name of [
    'taskDeclaredDeps', 'evaluateDepRecord', 'evaluateDependencyReadiness',
    'buildDependencyReadinessRationale', 'normalizeDependencyCollectorOutput',
  ]) {
    const redefined = new RegExp(`(?:const|let|var|function)\\s+${name}\\b`).test(w);
    assert.equal(redefined, false, `${name} must not be redefined in wiring block`);
  }
});

test('normalizer: structured object with deps array → dep-keyed record map', () => {
  const out = loadWiringApi().normalizeDependencyCollectorOutput(
    { deps: [{ dep: 'T1', committed: true, reachable: true, greenEvidence: true, commitSha: 'abc123' }] },
    ['T1'],
  );
  assert.deepEqual(out, {
    T1: { dep: 'T1', committed: true, reachable: true, greenEvidence: true, commitSha: 'abc123' },
  });
});

test('normalizer: depRecords object shape accepted as alternative form', () => {
  const out = loadWiringApi().normalizeDependencyCollectorOutput(
    { depRecords: { T1: { committed: true, reachable: true, greenEvidence: false } } },
    ['T1'],
  );
  assert.deepEqual(out, { T1: { committed: true, reachable: true, greenEvidence: false, dep: 'T1' } });
});

test('normalizer: fenced JSON string body parsed (fall-back path)', () => {
  const body = 'lead\n```json\n{"deps":[{"dep":"T1","committed":false,"reachable":false,"greenEvidence":false,"commitSha":null}]}\n```\ntrailer';
  const out = loadWiringApi().normalizeDependencyCollectorOutput(body, ['T1']);
  assert.deepEqual(out, { T1: { dep: 'T1', committed: false, reachable: false, greenEvidence: false, commitSha: null } });
});

test('normalizer: bare JSON containing "deps" parsed without fence', () => {
  const out = loadWiringApi().normalizeDependencyCollectorOutput(
    '{"deps":[{"dep":"T1","committed":true,"reachable":true,"greenEvidence":true}]}',
    ['T1'],
  );
  assert.ok(out && out.T1 && out.T1.greenEvidence === true);
});

test('normalizer: unparseable / missing collector output → null (fail-closed)', () => {
  const api = loadWiringApi();
  for (const bad of [
    null, undefined, '', 'collector crashed — no structured output', 42,
    { nope: 1 }, { deps: 'x' }, { deps: [] }, ['array'], { deps: [null, 's', 3] },
  ]) {
    assert.equal(api.normalizeDependencyCollectorOutput(bad, ['T1']), null, JSON.stringify(bad));
  }
});

test('normalizer: records pass through raw (strict flag validation is evaluateDepRecord job)', () => {
  const out = loadWiringApi().normalizeDependencyCollectorOutput(
    { deps: [{ dep: 'T1', committed: 'yes', reachable: 1, greenEvidence: null }] },
    ['T1'],
  );
  assert.deepEqual(out, { T1: { dep: 'T1', committed: 'yes', reachable: 1, greenEvidence: null } });
  const r = loadApi().evaluateDependencyReadiness({ id: 'T2', dependencies: ['T1'] }, out);
  assert.equal(r.verdict, 'unknown-evidence', 'non-boolean flags → downstream unknown-evidence');
});

test('normalizer: collector omitting a declared dep leaves it absent → downstream unknown-evidence', () => {
  const out = loadWiringApi().normalizeDependencyCollectorOutput(
    { deps: [{ dep: 'T0', committed: true, reachable: true, greenEvidence: true }] },
    ['T0', 'T1'],
  );
  assert.ok(out && Object.hasOwn(out, 'T0') && !Object.hasOwn(out, 'T1'));
  const r = loadApi().evaluateDependencyReadiness({ id: 'T3', dependencies: ['T0', 'T1'] }, out);
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'unknown-evidence');
});

test('composition: unparseable collector output + declared deps → gate blocks unknown-evidence (AC-2/AC-3)', () => {
  const records = loadWiringApi().normalizeDependencyCollectorOutput(
    'collector could not run',
    ['T1'],
  );
  const r = loadApi().evaluateDependencyReadiness({ id: 'T2', dependencies: ['T1'] }, records);
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'unknown-evidence');
});

test('composition: UC-2 collector output (dep uncommitted) → gate blocks dep-not-committed', () => {
  const records = loadWiringApi().normalizeDependencyCollectorOutput(
    { deps: [{ dep: 'T1', committed: false, reachable: false, greenEvidence: false, commitSha: null }] },
    ['T1'],
  );
  const r = loadApi().evaluateDependencyReadiness({ id: 'T2', dependencies: ['T1'] }, records);
  assert.equal(r.ready, false);
  assert.equal(r.verdict, 'dep-not-committed');
});

test('composition: UC-1 collector output (all verified) → gate ready → dispatch path taken', () => {
  const records = loadWiringApi().normalizeDependencyCollectorOutput(
    { deps: [
      { dep: 'T1', committed: true, reachable: true, greenEvidence: true, commitSha: 'abc' },
      { dep: 'T0', committed: true, reachable: true, greenEvidence: true, commitSha: 'def' },
    ] },
    ['T1', 'T0'],
  );
  const r = loadApi().evaluateDependencyReadiness({ id: 'T2', dependencies: ['T1', 'T0'] }, records);
  assert.equal(r.ready, true);
  assert.equal(r.verdict, 'ready');
});

test('schema: DEPENDENCY_COLLECTOR_SCHEMA requires per-dep boolean evidence fields', () => {
  const schema = loadWiringApi().DEPENDENCY_COLLECTOR_SCHEMA;
  assert.equal(schema.type, 'object');
  const item = schema.properties.deps.items;
  for (const field of ['dep', 'committed', 'reachable', 'greenEvidence']) {
    assert.ok(item.required.includes(field), `required field ${field}`);
  }
  assert.equal(item.properties.committed.type, 'boolean');
  assert.equal(item.properties.reachable.type, 'boolean');
  assert.equal(item.properties.greenEvidence.type, 'boolean');
  assert.equal(item.properties.dep.type, 'string');
});
