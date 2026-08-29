// TDD tests for Issue #99 Task 4: Implementation Engineer テンプレート分岐.
// estimate task の metadata.lane ('release'|'merge') で明示的に分岐し、
// merge 前提タスクには TDD テンプレートでなく実態調査型プロンプトを適用。
// パターンマッチ (operator-gated 判定で起きた偽陽性と同種) を使わない。
// lane metadata 欠損・不正値は fail-closed (TDD テンプレートへ黙墜ちしない)。
//
// Run: node --test .claude/workflows/tests/implement-lane-template.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [implement-lane-begin]';
const END = '// [implement-lane-end]';

const loadApi = () => {
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END);
  assert.ok(begin >= 0, 'missing implement-lane-begin marker');
  assert.ok(end > begin, 'missing implement-lane-end marker');
  const block = src.slice(begin + BEGIN.length, end);
  return new Function(
    `${block}; return { resolveImplementLane, buildEngineerPrompt };`
  )();
};

test('markers exist and pure block evaluates', () => {
  const api = loadApi();
  assert.equal(typeof api.resolveImplementLane, 'function');
  assert.equal(typeof api.buildEngineerPrompt, 'function');
});

// ── resolveImplementLane: 明示的 lane 判定 (パターンマッチ禁止) ──

test("lane 'merge' → investigation mode", () => {
  const r = loadApi().resolveImplementLane({ id: '1', metadata: { lane: 'merge' } });
  assert.deepEqual(r, { mode: 'investigation', lane: 'merge' });
});

test("lane 'release' → investigation mode", () => {
  const r = loadApi().resolveImplementLane({ id: '1', metadata: { lane: 'release' } });
  assert.deepEqual(r, { mode: 'investigation', lane: 'release' });
});

test("lane 'tdd' → tdd mode (明示的な TDD 宣言)", () => {
  const r = loadApi().resolveImplementLane({ id: '1', metadata: { lane: 'tdd' } });
  assert.deepEqual(r, { mode: 'tdd', lane: 'tdd' });
});

// ── fail-closed: metadata 欠損・不正値は TDD へ黙墜ちしない (approval 条件) ──

test('metadata missing → fail-closed, NOT tdd default', () => {
  for (const task of [{ id: '1' }, { id: '1', metadata: undefined }, { id: '1', metadata: null }]) {
    const r = loadApi().resolveImplementLane(task);
    assert.equal(r.mode, 'fail-closed', JSON.stringify(task));
    assert.equal(r.reason, 'missing-lane-metadata');
  }
});

test('lane key missing → fail-closed', () => {
  const r = loadApi().resolveImplementLane({ id: '1', metadata: { other: 1 } });
  assert.equal(r.mode, 'fail-closed');
  assert.equal(r.reason, 'missing-lane-metadata');
});

test('lane unknown value → fail-closed', () => {
  for (const lane of ['code', 'docs', '', 'MERGE', 42, null, undefined, {}]) {
    const r = loadApi().resolveImplementLane({ id: '1', metadata: { lane } });
    assert.equal(r.mode, 'fail-closed', `lane=${JSON.stringify(lane)}`);
    assert.equal(r.reason, 'unknown-lane');
  }
});

test('task null/malformed → fail-closed', () => {
  for (const task of [null, undefined, 42, 'x']) {
    const r = loadApi().resolveImplementLane(task);
    assert.equal(r.mode, 'fail-closed');
    assert.equal(r.reason, 'missing-lane-metadata');
  }
});

// ── buildEngineerPrompt: テンプレート内容 ──

test('investigation prompt: PR 状態確認・trunk-membership・merge/close 判定を含み TDD 指示を含まない', () => {
  const api = loadApi();
  const p = api.buildEngineerPrompt(
    { mode: 'investigation', lane: 'merge' },
    { id: '4', description: 'verify open PR' },
    { title: 'T' },
    { decision: 'APPROVE' }
  );
  assert.equal(typeof p, 'string');
  assert.match(p, /gh pr (view|status|list)/);
  assert.match(p, /git branch -a --contains/);
  assert.match(p, /merge|close/i);
  assert.doesNotMatch(p, /using TDD/);
  assert.doesNotMatch(p, /Write tests FIRST/);
  assert.doesNotMatch(p, /cargo fmt/);
});

test('tdd prompt: 従来の TDD 実装指示を含む', () => {
  const api = loadApi();
  const p = api.buildEngineerPrompt(
    { mode: 'tdd', lane: 'tdd' },
    { id: '1', description: 'impl' },
    { title: 'T' },
    { decision: 'APPROVE' }
  );
  assert.match(p, /using TDD/);
  assert.match(p, /Write tests FIRST/);
});

test('fail-closed → buildEngineerPrompt は null (prompt 黙墜ち防止)', () => {
  const p = loadApi().buildEngineerPrompt(
    { mode: 'fail-closed', reason: 'missing-lane-metadata' },
    { id: '1' },
    {},
    {}
  );
  assert.equal(p, null);
});

// ── wiring: pipeline の engineer dispatch が分岐を使っている ──

test('wiring: implement phase が resolveImplementLane を dispatch 前に呼ぶ', () => {
  const implIdx = src.indexOf('engineer:${task.id}');
  assert.ok(implIdx > 0, 'engineer agent call present');
  const before = src.slice(Math.max(0, implIdx - 3000), implIdx);
  assert.match(before, /resolveImplementLane\(/);
  assert.match(before, /buildEngineerPrompt\(/);
});

test('wiring: lane-missing はタスク結果として fail-closed 報告される', () => {
  assert.match(src, /status: 'lane-missing'/);
});
