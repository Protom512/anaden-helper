// TDD tests for Issue #91 Task 4: intent<->fact contrast check.
// Pure module test — imports review-gate-contrast.js directly (ESM, node --test).
//
// Run: node --test .claude/workflows/tests/review-gate-contrast.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { testContrast, extractChangedPaths } from '../review-gate-contrast.js';

const CONSISTENT_DIFF = `=== STAT ===
 .claude/workflows/review-gate.js | 10 +-
 .claude/workflows/review-gate-diff.js | 4 +
=== DIFF ===
diff --git a/.claude/workflows/review-gate.js b/.claude/workflows/review-gate.js
--- a/.claude/workflows/review-gate.js
+++ b/.claude/workflows/review-gate.js`;
const CONSISTENT_TITLE = 'feat(review-gate): diff injection for workflows';
const CONSISTENT_FILES = ['.claude/workflows/review-gate.js'];

test('consistent case: diff overlaps title keywords and design/task files', () => {
  const r = testContrast({
    ticketTitle: CONSISTENT_TITLE,
    designFiles: CONSISTENT_FILES,
    taskFiles: CONSISTENT_FILES,
    diff: CONSISTENT_DIFF,
  });
  assert.equal(r.consistent, true);
  assert.deepEqual(r.mismatches, []);
});

test('title-diff mismatch: no changed path overlaps any title keyword', () => {
  const r = testContrast({
    ticketTitle: 'feat(lexer): token span fix',
    designFiles: ['crates/tsql-lexer/src/lib.rs'],
    taskFiles: ['crates/tsql-lexer/src/token.rs'],
    diff: CONSISTENT_DIFF, // only .claude/workflows/* changed
  });
  assert.equal(r.consistent, false);
  assert.ok(r.mismatches.some((m) => m.kind === 'title-diff'), 'has title-diff mismatch');
});

test('design-tasks mismatch: diff overlaps title but not declared design/task files', () => {
  const r = testContrast({
    ticketTitle: 'feat(pipeline): workflows contrast gate',
    designFiles: ['crates/tsql-lexer/src/design.rs'],
    taskFiles: ['crates/tsql-lexer/src/task.rs'],
    diff: CONSISTENT_DIFF,
  });
  assert.equal(r.consistent, false);
  assert.ok(
    r.mismatches.some((m) => m.kind === 'design-tasks'),
    'has design-tasks mismatch'
  );
});

test('empty diff input: reported as mismatch (not crash), consistent=false', () => {
  const r = testContrast({
    ticketTitle: CONSISTENT_TITLE,
    designFiles: CONSISTENT_FILES,
    taskFiles: CONSISTENT_FILES,
    diff: '',
  });
  assert.equal(r.consistent, false);
  assert.ok(r.mismatches.length >= 1);
  assert.ok(r.mismatches.every((m) => m.kind === 'title-diff' || m.kind === 'design-tasks'));
});

test('extractChangedPaths pulls paths from diff --git lines and stat lines', () => {
  const paths = extractChangedPaths(CONSISTENT_DIFF);
  assert.ok(paths.includes('.claude/workflows/review-gate.js'));
  assert.ok(paths.includes('.claude/workflows/review-gate-diff.js'));
});
