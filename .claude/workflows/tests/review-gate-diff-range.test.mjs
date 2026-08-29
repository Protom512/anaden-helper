// TDD tests for Issue #91 Task T1: buildCommitRangeDiffInput —
// commit-range-diff fallback logic (S2-FP-1). Prefers working-tree diff when
// non-empty, falls back to HEAD~1..HEAD (merge-base variant configurable),
// else returns {mode:'fail-closed'}. Includes tree-hash capture in evidence.
//
// Run: node --test .claude/workflows/tests/review-gate-diff-range.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { buildCommitRangeDiffInput } from '../review-gate-diff.js';

const TREE_HASH = 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2';
const BASE = { treeHash: TREE_HASH };

test('working-tree diff non-empty -> mode working-tree, uses stat/diff/untracked', () => {
  const out = buildCommitRangeDiffInput({
    stat: ' file.js | 2 +-\n',
    diff: 'diff --git a/file.js b/file.js\n+x',
    untracked: '',
    rangeStat: ' other.js | 3 +\n',
    rangeDiff: 'diff --git a/other.js b/other.js\n+y',
    ...BASE,
  });
  assert.equal(out.mode, 'working-tree');
  assert.equal(out.stat, ' file.js | 2 +-\n');
  assert.equal(out.diff, 'diff --git a/file.js b/file.js\n+x');
  assert.equal(out.untracked, '');
  assert.equal(out.treeHash, TREE_HASH);
  assert.ok(!out.note || !out.note.includes('intent-to-add'));
});

test('working-tree empty + commit-range non-empty -> mode commit-range, uses rangeStat/rangeDiff', () => {
  const out = buildCommitRangeDiffInput({
    stat: '',
    diff: '',
    untracked: '',
    rangeStat: ' other.js | 3 +\n',
    rangeDiff: 'diff --git a/other.js b/other.js\n+y',
    ...BASE,
  });
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.stat, ' other.js | 3 +\n');
  assert.equal(out.diff, 'diff --git a/other.js b/other.js\n+y');
  assert.equal(out.treeHash, TREE_HASH);
});

test('both empty -> mode fail-closed with reason', () => {
  const out = buildCommitRangeDiffInput({
    stat: '',
    diff: '',
    untracked: '',
    rangeStat: '',
    rangeDiff: '',
    ...BASE,
  });
  assert.equal(out.mode, 'fail-closed');
  assert.ok(typeof out.reason === 'string' && out.reason.length > 0);
  assert.equal(out.treeHash, TREE_HASH);
});

test('untracked-only -> fail-closed with intent-to-add note', () => {
  const out = buildCommitRangeDiffInput({
    stat: '',
    diff: '',
    untracked: '?? new-file.js\n',
    rangeStat: '',
    rangeDiff: '',
    ...BASE,
  });
  assert.equal(out.mode, 'fail-closed');
  assert.ok(out.note && out.note.includes('intent-to-add'),
    'note must mention intent-to-add (git add -N)');
});

test('merge-base variant configurable via options.rangeVariant', () => {
  const out = buildCommitRangeDiffInput(
    {
      stat: '',
      diff: '',
      untracked: '',
      rangeStat: ' a.js | 1 +\n',
      rangeDiff: 'diff --git a/a.js b/a.js\n+z',
      ...BASE,
    },
    { rangeVariant: 'merge-base' }
  );
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.rangeVariant, 'merge-base');
  const def = buildCommitRangeDiffInput(
    { stat: '', diff: '', untracked: '', rangeStat: ' a.js | 1 +\n', rangeDiff: 'x', ...BASE }
  );
  assert.equal(def.rangeVariant, 'head-prev', 'default variant is head-prev');
});

test('missing treeHash is not fabricated', () => {
  const out = buildCommitRangeDiffInput({ stat: 'a', diff: 'd', untracked: '' });
  assert.ok(!('treeHash' in out) || out.treeHash == null);
});

test('whitespace-only diff is treated as empty (fail-closed)', () => {
  const out = buildCommitRangeDiffInput({
    stat: '   \n',
    diff: '\n\t ',
    untracked: '',
    rangeStat: '',
    rangeDiff: '',
    ...BASE,
  });
  assert.equal(out.mode, 'fail-closed');
});
