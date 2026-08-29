import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';

// P-006 (cycle-20): mergeResult hoist regression lock.
// cycle-20 (#69/PR #70): `let mergeResult` was declared inside the
// `else { ... }` block but referenced by the final return *outside* it —
// the workflow completed a real merge then died with
// ReferenceError: mergeResult is not defined. This test locks the fix:
// exactly one top-level declaration, none inside the else block, and the
// final return still references it.

const src = readFileSync(new URL('../feature-pipeline.js', import.meta.url), 'utf8');

test('mergeResult is declared exactly once at top level (hoisted, P-006)', () => {
  const decls = [...src.matchAll(/^\s*let\s+mergeResult\s*=\s*null\s*;?\s*$/gm)];
  assert.equal(decls.length, 1, `expected exactly 1 'let mergeResult = null' declaration, found ${decls.length}`);
});

test('no mergeResult declaration remains inside the Phase-7 else block', () => {
  // The Phase 7 header must NOT be followed by a declaration (the old bug shape).
  const phase7 = src.indexOf('Phase 7: Merge & Close');
  assert.ok(phase7 > 0, 'Phase 7 header not found');
  const window = src.slice(phase7, phase7 + 400);
  assert.ok(!/let\s+mergeResult/.test(window), 'mergeResult must not be re-declared in Phase 7');
});

test('final return still references mergeResult', () => {
  const ret = src.lastIndexOf('mergeResult,');
  assert.ok(ret > 0, 'final return must include mergeResult');
});
