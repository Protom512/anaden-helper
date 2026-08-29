// Tests for review-gate diff injection (Issue #78 Task 2 — TDD, written BEFORE impl).
// Run: node --test .claude/workflows/review-gate-diff.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { normalizeGateDiff, buildDiffSection, withDiffContext } from './review-gate-diff.js';

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
