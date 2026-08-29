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
    `${blk}\nreturn { evaluateTicketPrecheck, deriveSliceMetadata };`
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
