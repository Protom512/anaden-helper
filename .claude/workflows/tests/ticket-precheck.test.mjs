// TDD tests for Issue #99 Task 2: ticket-precheck pure functions.
//
// evaluateTicketPrecheck(declaredFiles, changedFiles) ->
//   { verdict: 'PASS' | 'FAIL', declared, changed, undeclared, missing }
//   - undeclared: changed but not declared (fail-closed on any entry)
//   - missing: declared but not changed (warning-level, still FAIL per fail-closed)
//   - UC-1: exact match -> PASS
//   - UC-2: mismatch -> FAIL + mismatched file list
//   - Edge: diff non-empty but declaration empty -> FAIL (fail-closed)
//   - Edge: normalize path separators and './' prefixes before comparing
//   - Edge: malformed input (null/non-array/blank entries) -> FAIL (fail-closed)
//
// deriveSliceMetadata(changedFiles) ->
//   { changedCrates: string[], diffKind: 'docs-only'|'code'|'mixed' }
//   - reuses classifyDiffKind from gate-diff-kind.js (no self-declaration)
//
// Run: node --test .claude/workflows/tests/ticket-precheck.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  evaluateTicketPrecheck,
  deriveSliceMetadata,
} from '../ticket-precheck.js';
import { classifyDiffKind } from '../gate-diff-kind.js';

// ── UC-1: PASS ──

test('UC-1: declared files match changed files exactly -> PASS', () => {
  const r = evaluateTicketPrecheck(
    ['crates/foo/src/lib.rs', 'docs/guide.md'],
    ['crates/foo/src/lib.rs', 'docs/guide.md'],
  );
  assert.equal(r.verdict, 'PASS');
  assert.deepEqual(r.undeclared, []);
  assert.deepEqual(r.missing, []);
});

test('order-independent match -> PASS', () => {
  const r = evaluateTicketPrecheck(
    ['a.rs', 'b.js'],
    ['b.js', 'a.rs'],
  );
  assert.equal(r.verdict, 'PASS');
});

test('path separator and ./ normalization -> PASS', () => {
  const r = evaluateTicketPrecheck(
    ['crates\\foo\\src\\lib.rs', './scripts/run.js'],
    ['crates/foo/src/lib.rs', 'scripts/run.js'],
  );
  assert.equal(r.verdict, 'PASS');
});

test('duplicate declared entries deduped -> PASS', () => {
  const r = evaluateTicketPrecheck(
    ['a.rs', 'a.rs'],
    ['a.rs'],
  );
  assert.equal(r.verdict, 'PASS');
});

// ── UC-2: FAIL + mismatched file list ──

test('UC-2: changed file not declared -> FAIL with undeclared list', () => {
  const r = evaluateTicketPrecheck(
    ['crates/foo/src/lib.rs'],
    ['crates/foo/src/lib.rs', 'crates/bar/src/lib.rs'],
  );
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['crates/bar/src/lib.rs']);
});

test('UC-2: declared file not changed -> FAIL with missing list', () => {
  const r = evaluateTicketPrecheck(
    ['a.rs', 'b.js'],
    ['a.rs'],
  );
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.missing, ['b.js']);
});

test('UC-2: both directions mismatched -> FAIL with both lists', () => {
  const r = evaluateTicketPrecheck(
    ['a.rs', 'b.js'],
    ['a.rs', 'c.rs'],
  );
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['c.rs']);
  assert.deepEqual(r.missing, ['b.js']);
});

// ── fail-closed edges ──

test('diff non-empty but declaration empty -> FAIL (fail-closed)', () => {
  const r = evaluateTicketPrecheck([], ['a.rs']);
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['a.rs']);
});

test('both empty -> PASS (no diff, nothing declared — vacuous-clean)', () => {
  const r = evaluateTicketPrecheck([], []);
  assert.equal(r.verdict, 'PASS');
});

test('null / non-array / malformed input -> FAIL (fail-closed)', () => {
  assert.equal(evaluateTicketPrecheck(null, ['a.rs']).verdict, 'FAIL');
  assert.equal(evaluateTicketPrecheck(['a.rs'], null).verdict, 'FAIL');
  assert.equal(evaluateTicketPrecheck('a.rs', ['a.rs']).verdict, 'FAIL');
  // malformed entry (blank / non-string) counts as content -> fail-closed
  assert.equal(evaluateTicketPrecheck(['', '   '], ['a.rs']).verdict, 'FAIL');
  assert.equal(evaluateTicketPrecheck(['a.rs', 42], ['a.rs']).verdict, 'FAIL');
});

test('reason string is non-empty on FAIL', () => {
  const r = evaluateTicketPrecheck(['a.rs'], ['a.rs', 'b.rs']);
  assert.equal(r.verdict, 'FAIL');
  assert.ok(typeof r.reason === 'string' && r.reason.length > 0);
  assert.ok(r.reason.includes('b.rs'));
});

// ── deriveSliceMetadata ──

test('deriveSliceMetadata: crates derived from crates/* paths', () => {
  const m = deriveSliceMetadata([
    'crates/foo/src/lib.rs',
    'crates/foo/src/main.rs',
    'crates/bar/src/lib.rs',
    'docs/x.md',
  ]);
  assert.deepEqual(m.changedCrates, ['bar', 'foo']);
});

test('deriveSliceMetadata: no crates -> empty array', () => {
  const m = deriveSliceMetadata(['docs/a.md', 'scripts/run.js']);
  assert.deepEqual(m.changedCrates, []);
});

test('deriveSliceMetadata: diffKind matches classifyDiffKind (reuse, not duplicate)', () => {
  const cases = [
    ['README.md', 'docs/g.md'],
    ['crates/foo/src/lib.rs'],
    ['README.md', 'crates/foo/src/lib.rs'],
    [],
    null,
  ];
  for (const c of cases) {
    assert.equal(deriveSliceMetadata(c).diffKind, classifyDiffKind(c));
  }
});

test('deriveSliceMetadata: malformed input fail-closed to code + no crates', () => {
  const m = deriveSliceMetadata(null);
  assert.equal(m.diffKind, 'code');
  assert.deepEqual(m.changedCrates, []);
});

test('deriveSliceMetadata: path separators normalized (windows-style crates\\foo)', () => {
  const m = deriveSliceMetadata(['crates\\foo\\src\\lib.rs']);
  assert.deepEqual(m.changedCrates, ['foo']);
});

// ── Issue #102 修正: mode='pre-implementation' (Request→Estimate 間) の挙動 ──
// 実装前のため declared-but-unchanged (missing) は正常。undeclared のみ FAIL。
test('pre-implementation mode: declared-but-unchanged files do not FAIL (Issue #102 regression)', () => {
  const r = evaluateTicketPrecheck(['a.js', 'b.js'], [], 'pre-implementation');
  assert.equal(r.verdict, 'PASS');
  assert.deepEqual(r.missing, ['a.js', 'b.js']);
  assert.equal(r.undeclared.length, 0);
  assert.match(r.reason, /pre-implementation/);
});

test('pre-implementation mode: undeclared changed file still FAILs', () => {
  const r = evaluateTicketPrecheck(['a.js'], ['x.js'], 'pre-implementation');
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['x.js']);
});

test('pre-implementation mode: empty declaration with actual diff still FAILs (fail-closed)', () => {
  const r = evaluateTicketPrecheck([], ['x.js'], 'pre-implementation');
  assert.equal(r.verdict, 'FAIL');
});

test('pre-implementation mode: empty declaration and empty diff FAILs (declaration required)', () => {
  const r = evaluateTicketPrecheck([], [], 'pre-implementation');
  assert.equal(r.verdict, 'FAIL');
});

test('strict mode (default): declared-but-unchanged still FAILs (gate-time semantics preserved)', () => {
  const r1 = evaluateTicketPrecheck(['a.js'], []);
  assert.equal(r1.verdict, 'FAIL');
  const r2 = evaluateTicketPrecheck(['a.js'], [], 'strict');
  assert.equal(r2.verdict, 'FAIL');
});
