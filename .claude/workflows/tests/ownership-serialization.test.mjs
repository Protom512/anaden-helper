// TDD tests for Issue #106 Task 1+2: file ownership map + conflict detection +
// dispatch serialization + wiring/evidence persistence (cycle-40 retrospective
// proposal 3/5).
//
// feature-pipeline.js exposes a pure block between
//   // [file-ownership-begin]  ...  // [file-ownership-end]
// containing:
//   computeFileOwnership(tasks)   — task.files[] から file→taskId[] の所有権マップ
//   findFileConflicts(tasks)      — 同一ファイルに触るタスク群を検出
//   resolveDispatchGroups(tasks)  — 重複タスクは直列チェーン、分離タスクは
//                                   同一グループで並列維持 (UC-1)。
//                                   UC-3: files 未宣言 ([]・フィールド欠損) は
//                                   fail-closed — status 'ownership-undeclared'。
//   buildOwnershipSerializationRationale(tasks, treeHash, runId, runTimestamp)
//                                 — .omc/logs/{run-id}/ownership-serialization.json
//                                   へ永続する判定根拠 payload (§2 evidence)。
// Wiring (Task 2, [ownership-serialization-wiring-begin/end]):
//   - implResults は resolveDispatchGroups(estimate.tasks) のグループ結合
//   - 'ownership-undeclared' は return で fail-closed 報告 (dispatch しない)
//   - 既存 lane 分岐 / operator-gated skip / precheck は回帰なし
//
// Run: node --test .claude/workflows/tests/ownership-serialization.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [file-ownership-begin]';
const END = '// [file-ownership-end]';

const loadApi = () => {
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END);
  assert.ok(begin >= 0, 'missing file-ownership-begin marker');
  assert.ok(end > begin, 'missing file-ownership-end marker');
  const block = src.slice(begin + BEGIN.length, end);
  return new Function(
    `${block}; return { computeFileOwnership, findFileConflicts, resolveDispatchGroups, buildOwnershipSerializationRationale, ownershipRelevantTasks };`
  )();
};

test('markers exist and pure block evaluates', () => {
  const api = loadApi();
  assert.equal(typeof api.computeFileOwnership, 'function');
  assert.equal(typeof api.findFileConflicts, 'function');
  assert.equal(typeof api.resolveDispatchGroups, 'function');
  assert.equal(typeof api.buildOwnershipSerializationRationale, 'function');
});

// ── computeFileOwnership ──

test('derives file→task[] map from declared files (UC-2 基礎)', () => {
  const tasks = [
    { id: '1', files: ['a.rs', 'b.rs'] },
    { id: '2', files: ['b.rs'] },
    { id: '3', files: ['c.md'] },
  ];
  const r = loadApi().computeFileOwnership(tasks);
  assert.deepEqual(r.ownership, {
    'a.rs': ['1'],
    'b.rs': ['1', '2'],
    'c.md': ['3'],
  });
  assert.deepEqual(r.undeclaredIds, []);
});

test('undeclared files ([] / missing / non-array) tracked separately, not mapped', () => {
  const tasks = [
    { id: '1', files: [] },
    { id: '2' },
    { id: '3', files: 'a.rs' },
    { id: '4', files: ['a.rs'] },
  ];
  const r = loadApi().computeFileOwnership(tasks);
  assert.deepEqual(r.ownership, { 'a.rs': ['4'] });
  assert.deepEqual(r.undeclaredIds, ['1', '2', '3']);
});

test('null / non-array input → empty ownership, fail-closed empty result', () => {
  for (const input of [null, undefined, 42, 'x', {}]) {
    const r = loadApi().computeFileOwnership(input);
    assert.deepEqual(r.ownership, {}, JSON.stringify(input));
    assert.deepEqual(r.undeclaredIds, [], JSON.stringify(input));
  }
});

test('path normalization: backslash separators treated like forward slashes', () => {
  const r = loadApi().computeFileOwnership([
    { id: '1', files: ['src\\foo.js'] },
    { id: '2', files: ['src/foo.js'] },
  ]);
  assert.deepEqual(r.ownership['src/foo.js'], ['1', '2']);
});

// ── findFileConflicts ──

test('detects file shared by multiple tasks (UC-1 core)', () => {
  const conflicts = loadApi().findFileConflicts([
    { id: '1', files: ['a.rs', 'b.rs'] },
    { id: '2', files: ['b.rs'] },
    { id: '3', files: ['c.md'] },
  ]);
  assert.equal(conflicts.length, 1);
  assert.equal(conflicts[0].file, 'b.rs');
  assert.deepEqual(conflicts[0].taskIds, ['1', '2']);
});

test('disjoint tasks → no conflicts', () => {
  const conflicts = loadApi().findFileConflicts([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['b.rs'] },
  ]);
  assert.deepEqual(conflicts, []);
});

test('transitive conflict via chained files groups all three tasks per file entry', () => {
  const conflicts = loadApi().findFileConflicts([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['a.rs', 'b.rs'] },
    { id: '3', files: ['b.rs'] },
  ]);
  const files = conflicts.map((c) => c.file).sort();
  assert.deepEqual(files, ['a.rs', 'b.rs']);
});

test('undeclared task reported as potential conflict with every other in-flight task (UC-3)', () => {
  const conflicts = loadApi().findFileConflicts([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: [] },
  ]);
  assert.equal(conflicts.length, 1);
  assert.equal(conflicts[0].file, null);
  assert.deepEqual(conflicts[0].taskIds, ['1', '2']);
});

// ── resolveDispatchGroups ──

test('UC-1: conflicting tasks serialized as chain, disjoint tasks kept parallel in one group', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['a.rs'] },
    { id: '3', files: ['c.md'] },
    { id: '4', files: ['d.md'] },
  ]);
  // conflicting pair → each its own sequential stage, in task order
  const serialStages = r.groups.filter((g) => !g.parallel);
  assert.deepEqual(serialStages.map((g) => g.tasks), [['1'], ['2']]);
  // disjoint tasks → single parallel group
  const parallelGroups = r.groups.filter((g) => g.parallel);
  assert.equal(parallelGroups.length, 1);
  assert.deepEqual(parallelGroups[0].tasks, ['3', '4']);
});

test('transitive conflict chain serializes all linked tasks in order', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['a.rs', 'b.rs'] },
    { id: '3', files: ['b.rs'] },
    { id: '4', files: ['z.md'] },
  ]);
  const serialStages = r.groups.filter((g) => !g.parallel);
  assert.deepEqual(serialStages.map((g) => g.tasks), [['1'], ['2'], ['3']]);
  const parallelGroups = r.groups.filter((g) => g.parallel);
  assert.deepEqual(parallelGroups.map((g) => g.tasks), [['4']]);
});

test('UC-3: undeclared-files task appended as serial stage at the tail, never parallel', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: [] },
    { id: '3', files: ['b.rs'] },
  ]);
  assert.deepEqual(r.undeclaredIds, ['2']);
  const last = r.groups[r.groups.length - 1];
  assert.equal(last.parallel, false);
  assert.deepEqual(last.tasks, ['2']);
  // 並列グループに紛れ込んでいない
  for (const g of r.groups) {
    if (g.parallel) assert.ok(!g.tasks.includes('2'));
  }
});

test('all-undeclared input → every task its own serial stage (fail-closed to serial)', () => {
  const r = loadApi().resolveDispatchGroups([{ id: '1' }, { id: '2' }, { id: '3' }]);
  assert.ok(r.groups.every((g) => !g.parallel));
  assert.deepEqual(r.groups.map((g) => g.tasks), [['1'], ['2'], ['3']]);
});

test('empty task list → empty groups', () => {
  const r = loadApi().resolveDispatchGroups([]);
  assert.deepEqual(r.groups, []);
  assert.deepEqual(r.undeclaredIds, []);
});

test('every task appears exactly once across groups', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['a.rs', 'x.js'] },
    { id: '3', files: ['x.js'] },
    { id: '4', files: ['m.md'] },
    { id: '5' },
  ]);
  const all = r.groups.flatMap((g) => g.tasks).sort();
  assert.deepEqual(all, ['1', '2', '3', '4', '5']);
});

// ── Task 2: status / duplicateFiles / serializedPairs ──

test('disjoint declared tasks → status ok, empty duplicates', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['a.rs'] },
    { id: '2', files: ['b.js'] },
  ]);
  assert.equal(r.status, 'ok');
  assert.deepEqual(r.duplicateFiles, []);
  assert.deepEqual(r.serializedPairs, []);
});

test('shared file → duplicateFiles + serializedPairs in input order', () => {
  const r = loadApi().resolveDispatchGroups([
    { id: '1', files: ['x.js', 's.rs'] },
    { id: '2', files: ['s.rs'] },
    { id: '3', files: ['y.md'] },
  ]);
  assert.equal(r.status, 'ok');
  assert.deepEqual(r.duplicateFiles, ['s.rs']);
  assert.deepEqual(r.serializedPairs, [['1', '2']]);
});

test('any undeclared task → status ownership-undeclared (fail-closed)', () => {
  for (const bad of [
    [{ id: '1' }],
    [{ id: '1', files: [] }],
    [{ id: '1', files: undefined }],
    [{ id: '1', files: 'a.rs' }],
    [{ id: '1', files: ['a'] }, { id: '2' }],
    null, undefined, 'x',
  ]) {
    const r = loadApi().resolveDispatchGroups(bad);
    assert.equal(r.status, 'ownership-undeclared', JSON.stringify(bad));
  }
});

// ── Task 2: buildOwnershipSerializationRationale (§2 evidence payload) ──

test('rationale carries groups, duplicates, pairs, taskFiles, treeHash, runId, runTimestamp', () => {
  const r = loadApi().buildOwnershipSerializationRationale(
    [
      { id: '1', files: ['a', 's'] },
      { id: '2', files: ['s'] },
    ],
    'deadbeef', 'run-x', '2026-08-30T00:00:00Z',
  );
  assert.equal(r.status, 'ok');
  assert.equal(r.runId, 'run-x');
  assert.equal(r.runTimestamp, '2026-08-30T00:00:00Z');
  assert.equal(r.recordedAt, '2026-08-30T00:00:00Z');
  assert.equal(r.treeHash, 'deadbeef');
  assert.deepEqual(r.duplicateFiles, ['s']);
  assert.deepEqual(r.serializedPairs, [['1', '2']]);
  assert.ok(Array.isArray(r.groups) && r.groups.length >= 2);
  assert.deepEqual(r.taskFiles, [
    { task: '1', files: ['a', 's'] },
    { task: '2', files: ['s'] },
  ]);
  assert.equal(r.issue, 106);
});

test('rationale for undeclared tasks records undeclaredTasks + unknown treeHash', () => {
  const r = loadApi().buildOwnershipSerializationRationale([{ id: '1' }], '', 'run-x', 'ts');
  assert.equal(r.status, 'ownership-undeclared');
  assert.deepEqual(r.undeclaredTasks, ['1']);
  assert.equal(r.treeHash, 'unknown');
});

// ── Task 2: wiring (regression-safe dispatch rewrite) ──

test('wiring: implResults built from resolveDispatchGroups groups (tdd-lane tasks only, Issue #134)', () => {
  assert.ok(
    /const\s+ownershipPlan\s*=\s*resolveDispatchGroups\(ownershipRelevantTasks\(estimate\.tasks\)\)/.test(src),
    'dispatch driven by resolveDispatchGroups(ownershipRelevantTasks(estimate.tasks)) — tdd-lane only',
  );
  assert.ok(
    /for\s*\(const\s+group\s+of\s+ownershipPlan\.groups\)/.test(src),
    'dispatch iterates ownershipPlan.groups (serialized per group)',
  );
});

test('wiring: flat pipeline(estimate.tasks, ...) dispatch is gone (code, not comments)', () => {
  const code = src.replace(/^\s*\/\/.*$/gm, ''); // strip line comments
  assert.ok(
    !/pipeline\(\s*estimate\.tasks\s*,/.test(code),
    'old flat parallel dispatch over estimate.tasks must be removed',
  );
});

test('wiring: fail-closed ownership-undeclared reported without dispatch', () => {
  assert.ok(
    src.includes("status: 'ownership-undeclared'"),
    "fail-closed return reports status 'ownership-undeclared'",
  );
});

test('wiring: evidence persisted to .omc/logs/{runId}/ownership-serialization.json', () => {
  assert.ok(
    src.includes('.omc/logs/${runId}/ownership-serialization.json'),
    'EVIDENCE PERSISTER must write ownership-serialization.json (pipeline-evidence-verification.md §2)',
  );
});

test('wiring: existing lane branch / operator-gated skip / precheck intact', () => {
  assert.ok(src.includes('resolveImplementLane(task)'), 'lane branch kept');
  assert.ok(src.includes('_operatorAction && !_codeChange'), 'operator-gated skip kept');
  assert.ok(src.includes('ticket-precheck'), 'ticket precheck kept');
});

// ── Issue #134: ownership 対象は tdd lane タスクのみ ──
test('ownershipRelevantTasks filters to tdd-lane tasks (Issue #134)', () => {
  const eval2 = loadApi().ownershipRelevantTasks;
  const tasks = [
    { id: 'T1', files: ['a.js'], metadata: { lane: 'tdd' } },
    { id: 'T2', files: [], metadata: { lane: 'merge' } },    // 検証系: files 無しは正当
    { id: 'T3', files: [], metadata: { lane: 'release' } },  // 検証系
  ];
  const relevant = eval2(tasks);
  assert.equal(relevant.length, 1);
  assert.equal(relevant[0].id, 'T1');
});

test('ownershipRelevantTasks treats lane-less tasks as tdd (fail-closed side)', () => {
  const eval2 = loadApi().ownershipRelevantTasks;
  const tasks = [
    { id: 'T1', files: ['a.js'] },  // metadata 無し → tdd 扱い (ownership 対象)
  ];
  const relevant = eval2(tasks);
  assert.equal(relevant.length, 1);
});

test('verification-only tickets with zero tdd tasks pass ownership (Issue #134 regression)', () => {
  // 全タスクが merge/release lane の検証チケットは ownership チェック対象が空 →
  // resolveDispatchGroups([]) は status 'ok' で dispatch 可能。
  const api = loadApi();
  const relevant = api.ownershipRelevantTasks([
    { id: 'T1', files: [], metadata: { lane: 'merge' } },
    { id: 'T2', files: [], metadata: { lane: 'release' } },
  ]);
  assert.equal(relevant.length, 0);
  const plan = api.resolveDispatchGroups(relevant);
  assert.equal(plan.status, 'ok');
});
