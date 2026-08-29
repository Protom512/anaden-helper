// TDD tests for Issue #91 (P-007) T2: Commit Gate diff fetcher must also
// collect commit-range diff (HEAD~1..HEAD / merge-base variant) + tree hash,
// and fail-closed (short-circuit the gate) when BOTH working-tree diff and
// commit-range diff are empty — instead of injecting an empty diff into
// reviewers (vacuous-GO prevention, pipeline-evidence-verification.md §2).
//
// Two layers:
//  - unit tests of the pure helpers in review-gate-diff.js (T1 dependency)
//  - source-scan of feature-pipeline.js (script is not importable)
//
// Run: node --test .claude/workflows/tests/gate-commit-range-diff.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  extractDiffSection,
  buildCommitRangeDiffInput,
} from '../review-gate-diff.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

// ── unit: extractDiffSection parses "=== NAME ===" delimited sections ──

test('extractDiffSection returns body between headers', () => {
  const raw = [
    '=== STAT ===',
    ' a/b | 2 +-',
    '=== DIFF ===',
    'diff --git a/b b/b',
    '=== UNTRACKED ===',
    '?? new.ts',
    '=== COMMIT-RANGE STAT ===',
    ' c/d | 1 +',
    '=== COMMIT-RANGE DIFF ===',
    'diff --git a/c/d b/c/d',
    '=== TREE HASH ===',
    'abc123',
  ].join('\n');
  assert.equal(extractDiffSection(raw, 'STAT'), 'a/b | 2 +-');
  assert.equal(extractDiffSection(raw, 'DIFF'), 'diff --git a/b b/b');
  assert.equal(extractDiffSection(raw, 'UNTRACKED'), '?? new.ts');
  assert.equal(extractDiffSection(raw, 'COMMIT-RANGE STAT'), 'c/d | 1 +');
  assert.equal(extractDiffSection(raw, 'COMMIT-RANGE DIFF'), 'diff --git a/c/d b/c/d');
  assert.equal(extractDiffSection(raw, 'TREE HASH'), 'abc123');
});

test('extractDiffSection: missing section -> "", last section -> rest, whitespace trimmed', () => {
  assert.equal(extractDiffSection('=== DIFF ===\nhello\n', 'DIFF'), 'hello');
  assert.equal(extractDiffSection('no headers here', 'DIFF'), '');
  assert.equal(extractDiffSection('=== DIFF ===\n   \n', 'DIFF'), '');
  assert.equal(extractDiffSection(null, 'DIFF'), '');
});

// ── unit: buildCommitRangeDiffInput fallback + fail-closed (T1, exercised for T2 wiring) ──

test('buildCommitRangeDiffInput: working-tree diff wins when non-empty', () => {
  const r = buildCommitRangeDiffInput({ stat: 's', diff: 'd', rangeDiff: 'c', treeHash: 'h' });
  assert.equal(r.mode, 'working-tree');
  assert.equal(r.diff, 'd');
  assert.equal(r.treeHash, 'h');
});

test('buildCommitRangeDiffInput: falls back to commit-range when working tree empty', () => {
  const r = buildCommitRangeDiffInput({ rangeStat: 'rs', rangeDiff: 'rd' }, { rangeVariant: 'merge-base' });
  assert.equal(r.mode, 'commit-range');
  assert.equal(r.diff, 'rd');
  assert.equal(r.stat, 'rs');
  assert.equal(r.rangeVariant, 'merge-base');
});

test('buildCommitRangeDiffInput: both empty -> fail-closed with reason', () => {
  const r = buildCommitRangeDiffInput({ untracked: '' });
  assert.equal(r.mode, 'fail-closed');
  assert.ok(r.reason, 'fail-closed decision carries a reason');
});

// ── source scan: feature-pipeline.js wiring (gate:fetch-diff, ~L589) ──

test('feature-pipeline fetch-diff prompt collects commit-range stat + diff', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  assert.ok(idx > 0, 'gate:fetch-diff agent exists');
  const body = fpSrc.slice(fpSrc.lastIndexOf('agent(', idx), idx);
  assert.ok(body.includes('git --no-pager diff HEAD~1..HEAD --stat'), 'commit-range stat');
  assert.ok(body.includes('git --no-pager diff HEAD~1..HEAD'), 'commit-range full diff');
  assert.ok(/merge-base/.test(body), 'merge-base variant for merged contexts');
  assert.ok(body.includes('COMMIT-RANGE STAT'), 'commit-range stat header');
  assert.ok(body.includes('COMMIT-RANGE DIFF'), 'commit-range diff header');
});

test('feature-pipeline fetch-diff prompt collects tree hash (git write-tree)', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const body = fpSrc.slice(fpSrc.lastIndexOf('agent(', idx), idx);
  assert.ok(body.includes('git write-tree'), 'tree hash command');
  assert.ok(body.includes('=== TREE HASH ==='), 'tree hash header');
});

test('feature-pipeline keeps R8 caps 28000/24000/30000 after commit-range addition', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const body = fpSrc.slice(fpSrc.lastIndexOf('agent(', idx), idx);
  assert.ok(body.includes('28000'), '28000 threshold in prompt');
  assert.ok(body.includes('24000'), '24000 DIFF cut in prompt');
  const after = fpSrc.slice(idx, idx + 2500);
  assert.ok(after.includes('30000'), '30000 total cap on GATE_DIFF');
});

test('feature-pipeline: fail-closed uses T1 helper and short-circuits before reviewer prompts', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const after = fpSrc.slice(idx, idx + 5000);
  assert.ok(after.includes('buildCommitRangeDiffInput'), 'wired to T1 helper');
  assert.ok(after.includes("mode === 'fail-closed'"), 'fail-closed branch checks mode');
  assert.ok(/return\s*\{/.test(after), 'fail-closed branch returns (no empty-diff injection)');
  assert.ok(after.includes('snapshotBranch'), 'snapshot branch preserved (resumable, S2 pattern)');
  const instrAt = fpSrc.indexOf('FEEDBACK_INSTRUCTION');
  const failAt = fpSrc.indexOf("mode === 'fail-closed'");
  assert.ok(failAt > 0 && failAt < instrAt, 'short-circuit precedes reviewer prompt build');
});

// ── drift guard: inlined helpers in feature-pipeline.js must behave identically
//    to the canonical review-gate-diff.js module (approver condition: compare
//    input/output pairs, not just source text) ──

test('drift guard: inlined extractDiffSection/buildCommitRangeDiffInput match canonical behavior', async () => {
  // extract the inlined function bodies from feature-pipeline.js and eval them
  // in a sandbox alongside the canonical module.
  const inlineSrc = fpSrc.slice(
    fpSrc.indexOf('function extractDiffSection'),
    fpSrc.indexOf('const diffFetch = await agent(')
  );
  const inline = new Function(`${inlineSrc}\nreturn { extractDiffSection, buildCommitRangeDiffInput };`)();

  const cases = [
    '=== STAT ===\ns1\n=== DIFF ===\nd1\n=== UNTRACKED ===\nu1\n=== COMMIT-RANGE STAT ===\nrs1\n=== COMMIT-RANGE DIFF ===\nrd1\n=== TREE HASH ===\nth1',
    '=== DIFF ===\nonly-diff',
    'no headers at all',
    '',
    '=== STAT ===\n  \n=== DIFF ===\nd\n',
  ];
  const sectionNames = ['STAT', 'DIFF', 'UNTRACKED', 'COMMIT-RANGE STAT', 'COMMIT-RANGE DIFF', 'TREE HASH'];
  for (const raw of cases) {
    for (const name of sectionNames) {
      assert.equal(
        inline.extractDiffSection(raw, name),
        extractDiffSection(raw, name),
        `extractDiffSection(${JSON.stringify(raw)}, ${name}) diverged from canonical`
      );
    }
  }

  const inputPairs = [
    { stat: 's', diff: 'd', rangeStat: 'rs', rangeDiff: 'rd', treeHash: 't' },
    { stat: '', diff: '', rangeStat: 'rs', rangeDiff: 'rd' },
    { stat: '', diff: '', rangeStat: '', rangeDiff: '', untracked: '?? x' },
    { stat: '', diff: '', rangeStat: '', rangeDiff: '' },
  ];
  for (const input of inputPairs) {
    assert.deepEqual(
      inline.buildCommitRangeDiffInput(input),
      buildCommitRangeDiffInput(input),
      `buildCommitRangeDiffInput(${JSON.stringify(input)}) diverged from canonical`
    );
  }
  assert.deepEqual(
    inline.buildCommitRangeDiffInput({ rangeDiff: 'x' }, { rangeVariant: 'merge-base' }),
    buildCommitRangeDiffInput({ rangeDiff: 'x' }, { rangeVariant: 'merge-base' }),
    'rangeVariant option diverged from canonical'
  );
});
