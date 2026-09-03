// Behavioral drift guard for Issue #99 Task 3: the inline copies of
// evaluateTicketPrecheck / deriveSliceMetadata in feature-pipeline.js must
// behave identically to the canonical ticket-precheck.js module across a
// broad input matrix (same pattern as gate-diff-kind-wiring drift guard).
//
// Run: node --test .claude/workflows/tests/ticket-precheck-drift.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  evaluateTicketPrecheck,
  evaluateIssuePremise,
  deriveSliceMetadata,
} from '../ticket-precheck.js';
import { classifyDiffKind } from '../gate-diff-kind.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

function extractInlinePureFns() {
  const beginMarker = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const endMarker = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  assert.ok(beginMarker > 0 && endMarker > beginMarker, 'wiring markers present');
  const begin = fpSrc.indexOf('\n', beginMarker) + 1;
  const end = fpSrc.lastIndexOf('\n', endMarker);
  let blk = fpSrc.slice(begin, end).replace(/^\s*\/\/.*$/gm, '');
  // Keep only the pure-function region (up to the scope-resolver agent call —
  // the rest is runtime wiring with top-level await that cannot be eval'd).
  const cut = blk.indexOf('const precheckScope');
  assert.ok(cut > 0, 'pure region boundary (precheckScope) found');
  blk = blk.slice(0, cut);
  // The inline block references classifyDiffKind (defined earlier in the
  // pipeline from the gate-diff-kind inline copy) — inject the canonical one.
  const factory = new Function(
    'classifyDiffKind',
    `${blk}\nreturn { evaluateTicketPrecheck, evaluateIssuePremise, deriveSliceMetadata };`
  );
  return factory(classifyDiffKind);
}

test('drift guard: inline evaluateTicketPrecheck matches canonical across input matrix', () => {
  const inline = extractInlinePureFns();
  const cases = [
    // UC-1 PASS shapes
    [['crates/foo/src/lib.rs', 'docs/guide.md'], ['crates/foo/src/lib.rs', 'docs/guide.md']],
    [['b.js', 'a.rs'], ['a.rs', 'b.js']], // order-independent
    [['crates\\foo\\src\\lib.rs', './scripts/run.js'], ['crates/foo/src/lib.rs', 'scripts/run.js']],
    [['a.rs', 'a.rs'], ['a.rs']],
    [[], []],
    // UC-2 FAIL shapes
    [['crates/foo/src/lib.rs'], ['crates/foo/src/lib.rs', 'crates/bar/src/lib.rs']],
    [['a.rs', 'b.js'], ['a.rs']],
    [['a.rs', 'b.js'], ['a.rs', 'c.rs']],
    [[], ['a.rs']],
    // fail-closed malformed
    [null, ['a.rs']],
    [['a.rs'], null],
    ['a.rs', ['a.rs']],
    [['', '   '], ['a.rs']],
    [['a.rs', 42], ['a.rs']],
  ];
  for (const [d, c] of cases) {
    assert.deepEqual(
      inline.evaluateTicketPrecheck(d, c),
      evaluateTicketPrecheck(d, c),
      `evaluateTicketPrecheck(${JSON.stringify(d)}, ${JSON.stringify(c)}) diverged from canonical`
    );
  }
});

test('drift guard: inline deriveSliceMetadata matches canonical (incl. classifyDiffKind reuse)', () => {
  const inline = extractInlinePureFns();
  const cases = [
    ['crates/foo/src/lib.rs', 'crates/foo/src/main.rs', 'crates/bar/src/lib.rs', 'docs/x.md'],
    ['docs/a.md', 'scripts/run.js'],
    ['README.md', 'docs/g.md'],
    ['README.md', 'crates/foo/src/lib.rs'],
    ['crates/foo/src/lib.rs'],
    ['crates\\win\\src\\a.rs'],
    [],
    null,
  ];
  for (const c of cases) {
    const m1 = inline.deriveSliceMetadata(c);
    const m2 = deriveSliceMetadata(c);
    assert.deepEqual(m1, m2, `deriveSliceMetadata(${JSON.stringify(c)}) diverged from canonical`);
    assert.equal(m1.diffKind, classifyDiffKind(c), 'diffKind must equal canonical classifyDiffKind');
  }
});

test('drift guard: inline evaluateIssuePremise matches canonical across input matrix', () => {
  const inline = extractInlinePureFns();
  const cases = [
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [] },
    { issueState: 'closed', linkedBranchesContainIssue: true, openPRs: [] },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 1 }] },
    { issueState: 'closed', linkedBranchesContainIssue: true, openPRs: [{ number: 1 }] },
    { issueState: 'closed', linkedBranchesContainIssue: false, openPRs: [] },
    null,
    undefined,
    'open',
    {},
    { issueState: 'bogus', linkedBranchesContainIssue: false, openPRs: [] },
    { issueState: 'open', linkedBranchesContainIssue: 'no', openPRs: [] },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: null },
    // Issue #150 (Task 2) new-field cases: ticketKind / subjectPrNumber.
    // UC-1 exemption: continuation + declared subject PR explains all open PRs.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    // numeric-string subjectPrNumber ("149") must normalize to 149 before match.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: '149' },
    // unrelated open PR remains after excluding the subject -> FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }, { number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    // declared subject not among openPRs (mismatch) -> FAIL (premise broken).
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    // declared subject but openPRs empty (already merged) -> FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: 149 },
    // new-implementation + open PR -> legacy duplicate FAIL (no exemption).
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }], ticketKind: 'new-implementation' },
    // branch-only continuation (no subjectPrNumber) + empty openPRs -> PASS.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation' },
    // malformed ticketKind (incl. null) -> fail-closed FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'bogus' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: null },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 42 },
    // malformed subjectPrNumber (non-numeric / zero / negative / null) -> FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 'abc' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 0 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: -5 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: null },
    // contradictory: subjectPrNumber without continuation ticketKind -> FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'new-implementation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], subjectPrNumber: 149 },
    // stale + exemption collision: stale keeps priority -> FAIL.
    { issueState: 'closed', linkedBranchesContainIssue: true, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    // backward compat: fields entirely absent + open PR -> legacy duplicate FAIL.
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }] },
  ];
  for (const c of cases) {
    assert.deepEqual(
      inline.evaluateIssuePremise(c),
      evaluateIssuePremise(c),
      `evaluateIssuePremise(${JSON.stringify(c)}) diverged from canonical`
    );
  }
});

// Issue #150 (Task 2): deepEqual alone cannot catch "both copies wrong the same
// way" — the new-field cases must ALSO produce the semantically expected verdict
// in BOTH the inline copy and the canonical module (UC-1..UC-4 of Issue #150).
test('drift guard: inline evaluateIssuePremise Issue #150 semantics — expected verdicts on both copies', () => {
  const inline = extractInlinePureFns();
  const cases = [
    // UC-1: continuation + subject PR explains every open PR -> PASS (exemption).
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'PASS'],
    // UC-1 variant: numeric-string "149" normalizes and matches -> PASS.
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: '149' }, 'PASS'],
    // UC-2: unrelated open PR remains after excluding subject -> FAIL duplicate.
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }, { number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    // UC-2: new-implementation + open PR -> legacy duplicate FAIL.
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }], ticketKind: 'new-implementation' }, 'FAIL'],
    // UC-3: subject declared but not among openPRs -> FAIL (premise broken).
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    // UC-3: subject declared but openPRs empty (merged) -> FAIL.
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    // UC-4: malformed ticketKind -> FAIL (fail-closed).
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'bogus' }, 'FAIL'],
    // UC-4: malformed subjectPrNumber -> FAIL (fail-closed).
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 'abc' }, 'FAIL'],
    // backward compat: fields absent + open PR -> legacy duplicate FAIL.
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }] }, 'FAIL'],
    // stale + exemption collision: stale keeps priority over exemption -> FAIL.
    [{ issueState: 'closed', linkedBranchesContainIssue: true, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
  ];
  for (const [c, expected] of cases) {
    const fromInline = inline.evaluateIssuePremise(c);
    const fromCanonical = evaluateIssuePremise(c);
    assert.equal(fromInline.verdict, expected, `inline verdict for ${JSON.stringify(c)}`);
    assert.equal(fromCanonical.verdict, expected, `canonical verdict for ${JSON.stringify(c)}`);
    assert.deepEqual(fromInline, fromCanonical, `inline/canonical divergence for ${JSON.stringify(c)}`);
  }
});
