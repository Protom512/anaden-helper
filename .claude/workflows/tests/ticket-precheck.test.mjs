import test from 'node:test';
import assert from 'node:assert/strict';
import {
  evaluateTicketPrecheck,
  deriveSliceMetadata,
} from '../ticket-precheck.js';
import { classifyDiffKind } from '../gate-diff-kind.js';

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
  assert.equal(evaluateTicketPrecheck(['', '   '], ['a.rs']).verdict, 'FAIL');
  assert.equal(evaluateTicketPrecheck(['a.rs', 42], ['a.rs']).verdict, 'FAIL');
});

test('reason string is non-empty on FAIL', () => {
  const r = evaluateTicketPrecheck(['a.rs'], ['a.rs', 'b.rs']);
  assert.equal(r.verdict, 'FAIL');
  assert.ok(typeof r.reason === 'string' && r.reason.length > 0);
  assert.ok(r.reason.includes('b.rs'));
});

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
  const r = evaluateTicketPrecheck([], [], 'pre-implementation');
  assert.equal(r.verdict, 'PASS');
});

test('strict mode: empty declaration and empty diff PASSes (vacuous-clean, both-empty semantics preserved)', () => {
  const r = evaluateTicketPrecheck([], [], 'strict');
  assert.equal(r.verdict, 'PASS');
});

test('strict mode (default): declared-but-unchanged still FAILs (gate-time semantics preserved)', () => {
  const r1 = evaluateTicketPrecheck(['a.js'], []);
  assert.equal(r1.verdict, 'FAIL');
  const r2 = evaluateTicketPrecheck(['a.js'], [], 'strict');
  assert.equal(r2.verdict, 'FAIL');
});

import { evaluateIssuePremise } from '../ticket-precheck.js';

test('issuePremise UC-1: open issue with no linked merged branches and no open PRs -> PASS', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [],
  });
  assert.equal(r.verdict, 'PASS');
  assert.ok(r.reason.includes('PASS'));
});

test('issuePremise UC-2: closed issue already merged into trunk -> FAIL (stale)', () => {
  const r = evaluateIssuePremise({
    issueState: 'closed',
    linkedBranchesContainIssue: true,
    openPRs: [],
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /stale/i);
});

test('issuePremise UC-3: existing open PR for same issue -> FAIL (duplicate)', () => {
  const r = evaluateIssuePremise({
    issueState: 'open',
    linkedBranchesContainIssue: false,
    openPRs: [{ number: 123, title: 'feat: same issue work' }],
  });
  assert.equal(r.verdict, 'FAIL');
  assert.match(r.reason, /duplicate/i);
});

test('issuePremise edge: closed issue without merged branches -> PASS (closed != merged)', () => {
  const r = evaluateIssuePremise({
    issueState: 'closed',
    linkedBranchesContainIssue: false,
    openPRs: [],
  });
  assert.equal(r.verdict, 'PASS');
});

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
  assert.match(r.reason, /exemption/i);
  assert.match(r.reason, /#149/);
});

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
