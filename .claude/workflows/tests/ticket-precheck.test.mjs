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

test('pre-implementation mode: empty declaration and empty diff PASSes (verification-only ticket, Issue #131)', () => {
  // 検証・棚卸し系 continuation チケット (files:[]) の正当ケース。
  // 空宣言×非空diff のみ FAIL (fail-closed は別テストで維持)。
  const r = evaluateTicketPrecheck([], [], 'pre-implementation');
  assert.equal(r.verdict, 'PASS');
});

test('strict mode: empty declaration and empty diff PASSes (vacuous-clean, both-empty semantics preserved)', () => {
  // JSDoc 仕様: both empty -> PASS (vacuous-clean; nothing to verify)。
  // gate 時の空 diff は GATE_DIFF_EMPTY fail-closed が別経路で担保する。
  const r = evaluateTicketPrecheck([], [], 'strict');
  assert.equal(r.verdict, 'PASS');
});

test('strict mode (default): declared-but-unchanged still FAILs (gate-time semantics preserved)', () => {
  const r1 = evaluateTicketPrecheck(['a.js'], []);
  assert.equal(r1.verdict, 'FAIL');
  const r2 = evaluateTicketPrecheck(['a.js'], [], 'strict');
  assert.equal(r2.verdict, 'FAIL');
});

// ── Issue #109 Task 1: evaluateIssuePremise (stale/duplicate detection) ──

import { evaluateIssuePremise } from '../ticket-precheck.js';

// UC-1 (normal): open issue, no merged work, no open PR -> PASS
test('issuePremise UC-1: open issue with no linked merged branches and no open PRs -> PASS', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
  });
  assert.equal(r.verdict, 'PASS');
  assert.ok(r.reason.includes('PASS'));
});

// UC-2 (normal, stale): issue closed and merged into trunk -> FAIL
test('issuePremise UC-2: closed issue already merged into trunk -> FAIL (stale)', () => {
  const r = evaluateIssuePremise({
    issueState: 'closed',
    linkedBranchesContainIssue: true,
    openPRs: [],
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /stale/i);
});

// UC-3 (normal, duplicate): open PR already exists for the same issue -> FAIL
test('issuePremise UC-3: existing open PR for same issue -> FAIL (duplicate)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 123, title: 'feat: same issue work' }],
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /duplicate/i);
});

// Edge 1: closed issue but NOT merged anywhere (e.g. closed as wontfix) — not
// merged, so not stale-by-merge; dispatch proceeds (verification is the
// wiring layer's concern; pure fn only rejects closed+merged)
test('issuePremise edge: closed issue without merged branches -> PASS (closed != merged)', () => {
  const r = evaluateIssuePremise({
    issueState: 'closed',
    linkedBranchesContainIssue: false,
    openPRs: [],
  });
  assert.equal(r.verdict, 'PASS');
});

// Edge 2 (fail-closed): null / malformed input -> FAIL
test('issuePremise edge: null or malformed input -> FAIL (fail-closed)', () => {
  assert.equal(evaluateIssuePremise(null).verdict, 'FAIL');
  assert.equal(evaluateIssuePremise(undefined).verdict, 'FAIL');
  assert.equal(evaluateIssuePremise('open').verdict, 'FAIL');
  assert.equal(evaluateIssuePremise({}).verdict, 'FAIL');
  assert.equal(
    evaluateIssuePremise({ issueState: 'bogus', linkedBranchesContainIssue: false, openPRs: [] }).verdict,
    'FAIL'
  );
});

// Edge 3 (fail-closed): invalid field types -> FAIL (gh failure/rate-limit
// surfaces as malformed input; never fail-open into stale dispatch)
test('issuePremise edge: invalid field types -> FAIL (fail-closed on gh failure shapes)', () => {
  assert.equal(
    evaluateIssuePremise({ issueState: 'open', linkedBranchesContainIssue: 'no', openPRs: [] }).verdict,
    'FAIL'
  );
  assert.equal(
    evaluateIssuePremise({ issueState: 'open', linkedBranchesContainIssue: false, openPRs: null }).verdict,
    'FAIL'
  );
  assert.equal(
    evaluateIssuePremise({ issueState: null, linkedBranchesContainIssue: false, openPRs: [] }).verdict,
    'FAIL'
  );
});

// ── Issue #150: continuation duplicate exemption in evaluateIssuePremise ──
//
// 入力契約拡張 (既存3フィールド不変、新規2フィールドは任意 — 不在 = 従来挙動):
//   - ticketKind: 'new-implementation' | 'continuation' (存在するが不正値 = FAIL)
//   - subjectPrNumber: 正整数または /^\d+$/ 数値文字列 (数値へ正規化)。
//     ticketKind='continuation' の文脈でのみ意味を持つ (矛盾宣言 = FAIL)。
//
// 判定: continuation + subject open PR が openPRs を完全に説明 (残り無し) のみ
// duplicate 例外 PASS。stale (closed+merged) は例外適用不可 (優先 FAIL)。

// UC-1 (修正主対象): continuation + subject PR が openPRs を完全に説明 -> PASS
test('issuePremise #150 UC-1: continuation + subject PR explains all openPRs -> PASS with exemption reason', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 149, title: 'feat: same issue work' }],
    ticketKind: 'continuation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'PASS');
  assert.equal(r.stale, false);
  // reason に exemption 根拠 (subject PR 番号) の明記を検証
  assert.match(r.reason, /exemption/i);
  assert.match(r.reason, /#149/);
});

// 数値文字列 subjectPrNumber は数値へ正規化されて一致判定される
test('issuePremise #150: numeric-string subjectPrNumber ("149") normalized to number for match', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 149 }],
    ticketKind: 'continuation',
    subjectPrNumber: '149',
  });
  assert.equal(r.verdict, 'PASS');
  assert.match(r.reason, /exemption/i);
});

// UC-2 (fail-closed 維持 その1): new-implementation + open PR -> 従来どおり FAIL
test('issuePremise #150 UC-2: new-implementation + open PR -> FAIL duplicate (legacy behavior)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 123 }],
    ticketKind: 'new-implementation',
  });
  assert.equal(r.verdict, 'FAIL');
  assert.equal(r.duplicate, true);
  assert.match(r.reason, /duplicate/i);
});

// UC-2 (fail-closed 維持 その2): ticketKind 不在 + open PR -> 従来どおり FAIL
test('issuePremise #150 UC-2: absent ticketKind + open PR -> FAIL duplicate (legacy behavior)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 123 }],
  });
  assert.equal(r.verdict, 'FAIL');
  assert.equal(r.duplicate, true);
  assert.match(r.reason, /duplicate/i);
});

// UC-2 系: continuation + subject 一致だが無関係 open PR が残る -> 残り PR への duplicate FAIL
test('issuePremise #150: continuation + subject match but unrelated open PR remains -> FAIL duplicate', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 149 }, { number: 200 }],
    ticketKind: 'continuation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.equal(r.duplicate, true);
  assert.match(r.reason, /duplicate/i);
});

// UC-3 (エッジ): 宣言 subject が openPRs に不在 (番号不一致) -> FAIL (premise broken)
test('issuePremise #150 UC-3: declared subject PR not among openPRs -> FAIL (premise broken)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 200 }],
    ticketKind: 'continuation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /subject/i);
});

// UC-3 (エッジ): openPRs 空 (subject PR は merge 済み) でも FAIL — 過剰例外禁止
test('issuePremise #150 UC-3: subject PR declared but openPRs empty (already merged) -> FAIL', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
    ticketKind: 'continuation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /subject/i);
});

// UC-4 (malformed その1): ticketKind 不正値 / null -> FAIL
test('issuePremise #150 UC-4: invalid ticketKind -> FAIL (fail-closed)', () => {
  for (const ticketKind of ['bogus', null, 'Continuation', 42]) {
    const r = evaluateIssuePremise({
      issueState: 'open',
      linkedBranchesContainIssue: false,
      openPRs: [],
      ticketKind,
    });
    assert.equal(r.verdict, 'FAIL', `ticketKind=${JSON.stringify(ticketKind)} must FAIL`);
    assert.match(r.reason, /ticketKind/i);
  }
});

// UC-4 (malformed その2): subjectPrNumber 非数値・負数・小数・ゼロ -> FAIL
test('issuePremise #150 UC-4: malformed subjectPrNumber -> FAIL (fail-closed)', () => {
  for (const subjectPrNumber of ['abc', '-5', 1.5, 0, '0', null, true, { n: 149 }]) {
    const r = evaluateIssuePremise({
      issueState: 'open',
      linkedBranchesContainIssue: false,
      openPRs: [{ number: 149 }],
      ticketKind: 'continuation',
      subjectPrNumber,
    });
    assert.equal(r.verdict, 'FAIL', `subjectPrNumber=${JSON.stringify(subjectPrNumber)} must FAIL`);
    assert.match(r.reason, /subjectPrNumber/i);
  }
});

// UC-4 (malformed その3): new-implementation + subjectPrNumber 宣言の矛盾入力 -> FAIL
test('issuePremise #150 UC-4: new-implementation + subjectPrNumber contradiction -> FAIL', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
    ticketKind: 'new-implementation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /contradict/i);
});

// UC-4 系: ticketKind 不在 + subjectPrNumber 宣言も矛盾 (continuation 文脈のみ有効)
test('issuePremise #150 UC-4: absent ticketKind + subjectPrNumber -> FAIL (subject only valid for continuation)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /contradict/i);
});

// 回帰保護: continuation + subjectPrNumber 不在 + openPRs 空 = ブランチのみ継続の
// 正規ケース -> 従来挙動 (PASS) を壊さないこと
test('issuePremise #150: continuation without subjectPrNumber + empty openPRs -> PASS (branch-only continuation)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
    ticketKind: 'continuation',
  });
  assert.equal(r.verdict, 'PASS');
  assert.equal(r.duplicate, false);
});

// stale 優先: closed+merged は continuation 例外適用不可 (exemption は duplicate のみ対象)
test('issuePremise #150: stale (closed+merged) takes priority over continuation exemption -> FAIL', () => {
  const r = evaluateIssuePremise({
    issueState: 'closed',
    linkedBranchesContainIssue: true,
    openPRs: [{ number: 149 }],
    ticketKind: 'continuation',
    subjectPrNumber: 149,
  });
  assert.equal(r.verdict, 'FAIL');
  assert.equal(r.stale, true);
  assert.match(r.reason, /stale/i);
  assert.doesNotMatch(r.reason, /exemption/i);
});
