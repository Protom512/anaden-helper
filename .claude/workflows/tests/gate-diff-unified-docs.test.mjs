// TDD tests for Issue #102 T6: documentation applied-notes.
//  (a) S2-GATE-FALSE-POSITIVE-FIX-PROPOSAL.md must carry an Issue #102
//      "applied" block of the same shape as the FP-1 P-007/#91 note:
//      an "適用済み" quote block referencing Issue #102, the deterministic
//      fallback chain, gate-diff.json persistence, and the approval trail
//      (estimate approval condition per the approval-conditions contract).
//  (b) .claude/rules/pipeline-evidence-verification.md §2 must reference the
//      gate-diff.json persistence (single source of truth snapshot) instead
//      of saying the fix is merely documented in the S2 proposal.
//
// Run: node --test .claude/workflows/tests/gate-diff-unified-docs.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const s2doc = readFileSync(
  join(repoRoot, '.claude', 'workflows', 'S2-GATE-FALSE-POSITIVE-FIX-PROPOSAL.md'),
  'utf8',
);
const rules = readFileSync(
  join(repoRoot, '.claude', 'rules', 'pipeline-evidence-verification.md'),
  'utf8',
);

test('S2 doc: Issue #102 applied block exists (same shape as P-007/#91 note)', () => {
  assert.ok(s2doc.includes('#102'), 'S2 doc must reference Issue #102');
  assert.ok(/適用済み.*#102|#102.*適用済み/.test(s2doc),
    'an applied (適用済み) note referencing Issue #102 must exist');
});

test('S2 doc: applied block describes deterministic fallback chain', () => {
  assert.ok(/HEAD~1\.\.HEAD/.test(s2doc));
  assert.ok(/merge-base/.test(s2doc));
  assert.ok(/intent-to-add|git add -N/.test(s2doc));
});

test('S2 doc: applied block mentions gate-diff.json persistence and fail-closed', () => {
  assert.ok(s2doc.includes('gate-diff.json'));
  assert.ok(/fail-closed/.test(s2doc));
});

test('S2 doc: applied block records approval trail (estimate approval)', () => {
  assert.ok(/承認/.test(s2doc), 'approval context must be recorded in the applied block');
});

test('rules §2: references gate-diff.json single-source persistence', () => {
  assert.ok(rules.includes('gate-diff.json'),
    'pipeline-evidence-verification.md §2 must mention gate-diff.json persistence');
});

test('rules §2: no longer claims the fix is documentation-only', () => {
  const sect2 = rules.split('## 2.')[1]?.split('## 3.')[0] ?? '';
  assert.ok(!/文書化済み。/.test(sect2) || sect2.includes('#102'),
    '§2 must not leave the fix as "documented only" without the #102 applied pointer');
});
