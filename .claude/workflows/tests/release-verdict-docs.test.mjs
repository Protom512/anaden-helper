// TDD tests for Issue #152 Task 4: documentation reconciliation (FP-2 applied note).
//  (a) S2-GATE-FALSE-POSITIVE-FIX-PROPOSAL.md FP-2 must carry an "applied (Issue #152)"
//      status of the same shape as the FP-1 P-007/#91 note: an 適用済み quote block
//      referencing Issue #152, the actually implemented verdict-persistence mechanism
//      (PR review COMMENT with a leading machine-readable VERDICT line,
//      .omc/logs/{run-id}/release-review-verdicts.md, gate-verdicts.json, and
//      consensus-actual-verdict interpolation in the release prompt), the wiring
//      tests, and the approval trail.
//  (b) .claude/rules/pipeline-evidence-verification.md §1.1 must add a
//      Release Review verdict / Commit Gate verdict row (PR review COMMENT or the
//      persisted verdict files) and codify that the release prompt gate description
//      must interpolate the consensus actual verdicts — fixed strings like
//      "all GO" are forbidden unless every lane is actually GO.
//
// Run: node --test .claude/workflows/tests/release-verdict-docs.test.mjs
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

// FP-2 section slice: between the FP-2 and FP-3 headings (FP-1's applied block
// must not satisfy FP-2 assertions).
const fp2 =
  s2doc.split('## FP-2:')[1]?.split('## FP-3:')[0] ??
  assert.fail('S2 doc must have an FP-2 section followed by an FP-3 section');
// §1.1 slice: between the 1.1 and 1.2 headings.
const sect11 =
  rules.split('### 1.1')[1]?.split('### 1.2')[0] ??
  assert.fail('rules doc must have a §1.1 section followed by §1.2');

test('S2 doc: FP-2 heading carries applied status (Issue #152)', () => {
  assert.match(s2doc, /## FP-2:[^\n]*applied \(Issue #152\)/,
    'FP-2 heading must carry "applied (Issue #152)" like FP-1 does for P-007/#91');
});

test('S2 doc: FP-2 applied note exists (適用済み block referencing Issue #152)', () => {
  assert.match(fp2, /適用済み[^\n]*#152|#152[^\n]*適用済み/,
    'an applied (適用済み) note referencing Issue #152 must exist inside FP-2');
});

test('S2 doc: FP-2 applied note references the actual persistence mechanism', () => {
  assert.ok(fp2.includes('release-review-verdicts.md'),
    'FP-2 applied note must reference .omc/logs/{run-id}/release-review-verdicts.md');
  assert.ok(fp2.includes('gate-verdicts.json'),
    'FP-2 applied note must reference .omc/logs/{run-id}/gate-verdicts.json');
  assert.ok(/COMMENT/.test(fp2),
    'FP-2 applied note must state the PR review COMMENT form (not --approve)');
  assert.ok(/VERDICT:/.test(fp2),
    'FP-2 applied note must reference the leading machine-readable VERDICT line');
  assert.ok(/release-verdict-persistence\.test\.mjs/.test(fp2),
    'FP-2 applied note must point at the persistence wiring tests');
});

test('S2 doc: FP-2 applied note covers release prompt interpolation (fixed all-GO forbidden)', () => {
  assert.ok(/interpolation/.test(fp2),
    'FP-2 applied note must describe consensus-actual-verdict interpolation');
  assert.ok(/全 lane GO/.test(fp2),
    'FP-2 applied note must restrict "all GO" to the all-lanes-GO case');
});

test('S2 doc: FP-2 applied note records approval trail (estimate approval)', () => {
  assert.ok(/承認/.test(fp2),
    'approval context must be recorded in the FP-2 applied note (same shape as FP-1)');
});

test('rules §1.1: Release Review / Commit Gate verdict evidence row exists', () => {
  const row =
    sect11.split('\n').find((l) => l.includes('Release Review verdict')) ?? '';
  assert.notEqual(row, '', '§1.1 table must have a Release Review verdict row');
  assert.ok(row.includes('Commit Gate verdict'),
    'the row must also cover Commit Gate verdicts');
  assert.ok(row.includes('COMMENT'),
    'the row must name the PR review COMMENT form');
  assert.ok(/VERDICT/.test(row),
    'the row must require the leading machine-readable VERDICT line');
  assert.ok(row.includes('release-review-verdicts.md') && row.includes('gate-verdicts.json'),
    'the row must name the persisted verdict files as the alternative evidence form');
  assert.ok(row.includes('#152'),
    'the row must carry the Issue #152 applied pointer');
});

test('rules §1.1: release prompt gate description must interpolate consensus verdicts', () => {
  assert.ok(/interpolation/.test(sect11),
    '§1.1 must codify consensus-actual-verdict interpolation for the release prompt');
  assert.ok(/固定文字列/.test(sect11),
    '§1.1 must forbid fixed-string gate descriptions');
  assert.ok(/全 lane GO/.test(sect11),
    '§1.1 must restrict "all GO" output to the all-lanes-GO case');
});

test('rules §1.1: fail-closed fallback when PR review posting fails', () => {
  assert.ok(/fallback/.test(sect11) && /fail-closed/.test(sect11),
    '§1.1 must state the persistent-log fallback so evidence is never lost (fail-closed)');
});
