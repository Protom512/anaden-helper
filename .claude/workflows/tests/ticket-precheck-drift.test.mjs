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
  const cut = blk.indexOf('const precheckScope');
  assert.ok(cut > 0, 'pure region boundary (precheckScope) found');
  blk = blk.slice(0, cut);
  const factory = new Function(
    'classifyDiffKind',
    `${blk}\nreturn { evaluateTicketPrecheck, evaluateIssuePremise, deriveSliceMetadata };`
  );
  return factory(classifyDiffKind);
}

test('drift guard: inline evaluateTicketPrecheck matches canonical across input matrix', () => {
  const inline = extractInlinePureFns();
  const cases = [
    [['crates/foo/src/lib.rs', 'docs/guide.md'], ['crates/foo/src/lib.rs', 'docs/guide.md']],
    [['b.js', 'a.rs'], ['a.rs', 'b.js']],
    [['crates\\foo\\src\\lib.rs', './scripts/run.js'], ['crates/foo/src/lib.rs', 'scripts/run.js']],
    [['a.rs', 'a.rs'], ['a.rs']],
    [[], []],
    [['crates/foo/src/lib.rs'], ['crates/foo/src/lib.rs', 'crates/bar/src/lib.rs']],
    [['a.rs', 'b.js'], ['a.rs']],
    [['a.rs', 'b.js'], ['a.rs', 'c.rs']],
    [[], ['a.rs']],
    [null, ['a.rs']],
    [['a.rs'], null],
    ['a.rs', ['a.rs']],
    [['', '   '], ['a.rs']],
    [['a.rs', 42], ['a.rs']],
  ];
  for (const mode of ['strict', 'pre-implementation']) {
    for (const [d, c] of cases) {
      assert.deepEqual(
        inline.evaluateTicketPrecheck(d, c, mode),
        evaluateTicketPrecheck(d, c, mode),
        `evaluateTicketPrecheck(${JSON.stringify(d)}, ${JSON.stringify(c)}, '${mode}') diverged from canonical`
      );
    }
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
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: '149' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }, { number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }], ticketKind: 'new-implementation' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'bogus' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: null },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 42 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 'abc' },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 0 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: -5 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: null },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'new-implementation', subjectPrNumber: 149 },
    { issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], subjectPrNumber: 149 },
    { issueState: 'closed', linkedBranchesContainIssue: true, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 },
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

test('drift guard: inline evaluateIssuePremise Issue #150 semantics — expected verdicts on both copies', () => {
  const inline = extractInlinePureFns();
  const cases = [
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'PASS'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: '149' }, 'PASS'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }, { number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }], ticketKind: 'new-implementation' }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 200 }], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'continuation', subjectPrNumber: 149 }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [], ticketKind: 'bogus' }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 149 }], ticketKind: 'continuation', subjectPrNumber: 'abc' }, 'FAIL'],
    [{ issueState: 'open', linkedBranchesContainIssue: false, openPRs: [{ number: 123 }] }, 'FAIL'],
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
