// Tests for review-gate diff injection (Issue #78 Task 2 — TDD, written BEFORE impl).
// Run: node --test .claude/workflows/review-gate-diff.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import {
  normalizeGateDiff,
  buildDiffSection,
  withDiffContext,
  buildUnifiedGateDiff,
} from './review-gate-diff.js';

const here = dirname(fileURLToPath(import.meta.url));

// ── normalizeGateDiff: R8 caps consistent with feature-pipeline.js ──

test('normalizeGateDiff: passes through string diff', () => {
  assert.equal(normalizeGateDiff('=== STAT ===\n...'), '=== STAT ===\n...');
});

test('normalizeGateDiff: null/undefined → empty string', () => {
  assert.equal(normalizeGateDiff(null), '');
  assert.equal(normalizeGateDiff(undefined), '');
});

test('normalizeGateDiff: non-string agent result → JSON stringified', () => {
  assert.equal(normalizeGateDiff({ stat: 'x' }), '{"stat":"x"}');
});

test('normalizeGateDiff: caps at 30000 chars total (R8 slice)', () => {
  const big = 'x'.repeat(50000);
  const out = normalizeGateDiff(big);
  assert.equal(out.length, 30000);
});

// ── buildDiffSection: === WORKING-TREE DIFF === block ──

test('buildDiffSection: wraps diff in begin/end markers', () => {
  const s = buildDiffSection('diff --git a/foo b/foo');
  assert.ok(s.includes('=== WORKING-TREE DIFF ==='));
  assert.ok(s.includes('diff --git a/foo b/foo'));
  assert.ok(s.includes('=== END DIFF ==='));
});

test('buildDiffSection: empty diff yields empty section (fail-open, keep gate runnable)', () => {
  assert.equal(buildDiffSection(''), '');
});

// ── withDiffContext: R8 instruction injection into reviewer prompts ──

test('withDiffContext: appends R8 instruction + diff section to prompt', () => {
  const out = withDiffContext('Architecture Review:', buildDiffSection('+foo'));
  assert.ok(out.startsWith('Architecture Review:'));
  assert.ok(out.includes('提供済み diff で直接分析'));
  assert.ok(out.includes('ファイルを個別に Read して再取得')); // do-not-reRead instruction
  assert.ok(out.includes('+foo'));
});

// ── wiring: review-gate.js actually injects diff into all 3 reviewer prompts ──

const src = readFileSync(join(here, 'review-gate.js'), 'utf8');

test('review-gate.js: fetches diff via R8-style agent before reviews', () => {
  assert.ok(src.includes('review:fetch-diff'));
  assert.ok(src.includes('DIFF_FETCH_PROMPT'));
  const mod = readFileSync(join(here, 'review-gate-diff.js'), 'utf8');
  assert.ok(mod.includes('git --no-pager diff HEAD'));
  assert.ok(mod.includes('24000')); // DIFF truncation threshold (R8 values)
  assert.ok(mod.includes('28000')); // total diff threshold (R8 values)
});

test('review-gate.js: all 3 reviewer prompts wrapped with withDiffContext (definition excluded)', () => {
  const def = (src.match(/^function withDiffContext\(/gm) || []).length;
  const all = (src.match(/withDiffContext\(/g) || []).length;
  assert.equal(all - def, 3);
});

test('review-gate.js: changed files list retained in reviewer prompts', () => {
  assert.ok(src.includes('Changed files: ${changes.changedFiles.join'));
});

// ── buildUnifiedGateDiff: deterministic fallback chain (Issue #102 UC-1 / T1) ──
// Chain: (a) working-tree non-empty → working-tree mode,
//        (b) else HEAD~1..HEAD commit-range,
//        (c) else merge-base (origin/master...HEAD),
//        (d) else untracked-only → intent-to-add mode,
//        (e) all empty or 429 placeholder → fail-closed.

test('buildUnifiedGateDiff: (a) non-empty working-tree STAT/DIFF → working-tree mode', () => {
  const out = buildUnifiedGateDiff({
    stat: ' foo.js | 2 ++',
    diff: 'diff --git a/foo.js b/foo.js',
    headPrevStat: ' bar.js | 1 +',
    headPrevDiff: 'diff --git a/bar.js b/bar.js',
    treeHash: 'abc123',
  });
  assert.equal(out.mode, 'working-tree');
  assert.equal(out.basis, 'working-tree');
  assert.equal(out.treeHash, 'abc123');
  assert.ok(out.snapshot.includes('=== STAT ==='));
  assert.ok(out.snapshot.includes('diff --git a/foo.js'));
  assert.ok(!out.snapshot.includes('bar.js')); // must not fall through
});

test('buildUnifiedGateDiff: (b) empty working-tree → falls back to HEAD~1..HEAD', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '',
    headPrevStat: ' bar.js | 1 +',
    headPrevDiff: 'diff --git a/bar.js b/bar.js',
    treeHash: 'def456',
  });
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.basis, 'commit-range:HEAD~1..HEAD');
  assert.ok(out.snapshot.includes('diff --git a/bar.js'));
});

test('buildUnifiedGateDiff: (c) HEAD~1..HEAD empty too → merge-base fallback', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: ' baz.js | 3 +++',
    mergeBaseDiff: 'diff --git a/baz.js b/baz.js',
    treeHash: null,
  });
  assert.equal(out.mode, 'commit-range');
  assert.equal(out.basis, 'commit-range:origin/master...HEAD');
  assert.ok(out.snapshot.includes('diff --git a/baz.js'));
  assert.equal('treeHash' in out, false); // null treeHash omitted
});

test('buildUnifiedGateDiff: (d) untracked only → intent-to-add mode with file list', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: '', mergeBaseDiff: '',
    untracked: '?? new-file.js\n?? other.js',
    treeHash: 'aaa111',
  });
  assert.equal(out.mode, 'intent-to-add');
  assert.equal(out.basis, 'untracked-only');
  assert.deepEqual(out.untrackedFiles, ['new-file.js', 'other.js']);
  assert.ok(out.snapshot.includes('new-file.js'));
  assert.ok(/individual Read|intent-to-add/i.test(out.snapshot));
});

test('buildUnifiedGateDiff: (e) everything empty → fail-closed', () => {
  const out = buildUnifiedGateDiff({
    stat: '', diff: '', headPrevStat: '', headPrevDiff: '',
    mergeBaseStat: '', mergeBaseDiff: '', untracked: '',
  });
  assert.equal(out.mode, 'fail-closed');
  assert.equal(out.basis, 'all-empty');
  assert.ok(typeof out.reason === 'string' && out.reason.length > 0);
  assert.equal(out.snapshot, '');
});

test('buildUnifiedGateDiff: 429 placeholder exact match in diff → fail-closed', () => {
  for (const ph of ['429', 'rate limit', 'placeholder']) {
    const out = buildUnifiedGateDiff({
      stat: '', diff: ph, headPrevStat: '', headPrevDiff: '',
      mergeBaseStat: '', mergeBaseDiff: '', untracked: '',
    });
    assert.equal(out.mode, 'fail-closed', `placeholder "${ph}" must be fail-closed`);
    assert.ok(/429|rate limit|placeholder/.test(out.reason));
    assert.equal(out.snapshot, '');
  }
});

test('buildUnifiedGateDiff: legitimate diff containing "placeholder" as substring is NOT fail-closed', () => {
  // Exact-match-only detection: a real diff touching a placeholder variable
  // must not trigger fail-closed (estimate approval condition #1).
  const out = buildUnifiedGateDiff({
    stat: ' ph.js | 2 ++',
    diff: 'diff --git a/ph.js b/ph.js\n-const placeholder = 1;\n+const placeholder = 2;',
  });
  assert.equal(out.mode, 'working-tree');
});

test('buildUnifiedGateDiff: missing input object → fail-closed (no throw)', () => {
  const out = buildUnifiedGateDiff(null);
  assert.equal(out.mode, 'fail-closed');
});

test('buildUnifiedGateDiff: reuses buildCommitRangeDiffInput semantics — working-tree still wins with stat only', () => {
  const out = buildUnifiedGateDiff({ stat: ' x | 1 +' });
  assert.equal(out.mode, 'working-tree');
});
