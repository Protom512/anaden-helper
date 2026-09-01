// TDD tests for Issue #91 Task T1: buildCommitRangeDiffInput —
// commit-range-diff fallback logic (S2-FP-1). Prefers working-tree diff when
// non-empty, falls back to HEAD~1..HEAD (merge-base variant configurable),
// else returns {mode:'fail-closed'}. Includes tree-hash capture in evidence.
//
// Run: node --test .claude/workflows/tests/review-gate-diff-range.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  buildCommitRangeDiffInput,
  buildUnifiedGateDiff,
} from '../review-gate-diff.js';

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

// ── Issue #102 T5: buildUnifiedGateDiff merge-base / intent-to-add / 429
//    placeholder cases (deterministic fallback chain, UC-1) ──

test('unified: merge-base fallback when working-tree and HEAD~1..HEAD are both empty', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', untracked: '',
    headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: ' x.js | 4 +++\n',
    mergeBaseDiff: 'diff --git a/x.js b/x.js\n+q',
    treeHash: TREE_HASH,
  });
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.basis, 'commit-range:origin/master...HEAD');
  assert.equal(out.rangeVariant, 'merge-base');
  assert.equal(out.treeHash, TREE_HASH);
  assert.ok(out.snapshot.includes('diff --git a/x.js'));
});

test('unified: merge-base preferred over intent-to-add when both available', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', untracked: '?? n.js',
    headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: ' y.js | 1 +',
    mergeBaseDiff: 'diff --git a/y.js b/y.js',
  });
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.basis, 'commit-range:origin/master...HEAD');
});

test('unified: HEAD~1..HEAD preferred over merge-base (chain order)', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', untracked: '',
    headPrevStat: ' a.js | 1 +', headPrevDiff: 'diff --git a/a.js b/a.js',
    mergeBaseStat: ' b.js | 9 +', mergeBaseDiff: 'diff --git a/b.js b/b.js',
  });
  assert.equal(out.basis, 'commit-range:HEAD~1..HEAD');
  assert.ok(!out.snapshot.includes('b.js | 9'), 'merge-base content must not leak');
});

test('unified: intent-to-add mode for untracked-only input', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', untracked: '?? foo.js\n?? bar/baz.js',
    headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '',
    treeHash: TREE_HASH,
  });
  assert.equal(out.mode, 'intent-to-add');
  assert.equal(out.basis, 'untracked-only');
  assert.deepEqual(out.untrackedFiles, ['foo.js', 'bar/baz.js']);
  assert.ok(out.snapshot.includes('foo.js') && out.snapshot.includes('bar/baz.js'));
  assert.ok(/Read each file individually|intent-to-add/.test(out.snapshot));
  assert.equal(out.treeHash, TREE_HASH);
});

test('unified: 429 placeholder exact-match (diff) -> fail-closed, empty snapshot', () => {
  for (const ph of ['429', 'rate limit', 'placeholder']) {
    const out = buildUnifiedGateDiff({
      stat: '', diff: ph, untracked: '',
      headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '',
    });
    assert.equal(out.mode, 'fail-closed', `placeholder "${ph}"`);
    assert.equal(out.basis, '429-placeholder');
    assert.equal(out.snapshot, '');
    assert.match(out.reason, /429|rate-limit|placeholder/);
  }
});

test('unified: 429 placeholder exact-match (merge-base sections) -> fail-closed', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', untracked: '',
    headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: 'rate limit', mergeBaseDiff: '',
  });
  assert.equal(out.mode, 'fail-closed');
  assert.equal(out.basis, '429-placeholder');
});

test('unified: placeholder as SUBSTRING of a legitimate diff is NOT fail-closed', () => {
  const out = buildUnifiedGateDiff({
    stat: ' p.js | 2 ++',
    diff: 'diff --git a/p.js b/p.js\n-const placeholder = 1;\n+const placeholder = 2;\n// rate limit note',
    untracked: '',
    headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '',
  });
  assert.equal(out.mode, 'working-tree');
  assert.notEqual(out.mode, 'fail-closed');
});

test('unified: whitespace-padded placeholder still exact-match fail-closed (trim)', () => {
  const out = buildUnifiedGateDiff({
    stat: '  429  ', diff: '', untracked: '',
    headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '',
  });
  assert.equal(out.mode, 'fail-closed');
});
