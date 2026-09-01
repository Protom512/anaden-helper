// TDD tests for Issue #102 T4: review-gate.js must mirror the T1 unified
// gate-diff helper (review-gate-diff.js buildUnifiedGateDiff) as an inline
// copy — same fallback chain (working-tree → HEAD~1..HEAD → merge-base →
// intent-to-add → fail-closed), same exact-match 429-placeholder detection,
// same fail-closed behavior — and its DIFF_FETCH_PROMPT must collect the
// merge-base range and untracked enumeration.
//
// Source-scan + behavior drift-guard pattern (review-gate.js is not importable:
// top-level phase()/agent() calls; Workflow runtime rejects ESM imports).
//
// Run: node --test .claude/workflows/tests/review-gate-unified-diff.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { buildUnifiedGateDiff as canonical } from '../review-gate-diff.js';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'review-gate.js');
const src = readFileSync(wfPath, 'utf8');

/** Extract a top-level `function name(...) {...}` source by brace matching. */
function extractFunctionSource(scriptSrc, name) {
  const start = scriptSrc.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `function ${name} must be inlined in review-gate.js`);
  const parenOpen = scriptSrc.indexOf('(', start);
  let parenDepth = 0;
  let bodyOpen = -1;
  for (let i = parenOpen; i < scriptSrc.length; i++) {
    const c = scriptSrc[i];
    if (c === '(') parenDepth += 1;
    else if (c === ')') {
      parenDepth -= 1;
      if (parenDepth === 0) { bodyOpen = scriptSrc.indexOf('{', i); break; }
    }
  }
  assert.ok(bodyOpen > 0, `parameter list not found for ${name}`);
  let depth = 0;
  let inStr = null;
  for (let i = bodyOpen; i < scriptSrc.length; i++) {
    const c = scriptSrc[i];
    if (inStr) {
      if (c === '\\') i += 1;
      else if (c === inStr) inStr = null;
      continue;
    }
    if (c === "'" || c === '"' || c === '`') { inStr = c; continue; }
    if (c === '{') depth += 1;
    else if (c === '}') {
      depth -= 1;
      if (depth === 0) return scriptSrc.slice(start, i + 1);
    }
  }
  assert.fail(`unbalanced braces extracting ${name}`);
}

/** Load buildUnifiedGateDiff + its private deps as one evaluatable bundle. */
function loadInlinedUnified() {
  const names = ['isPlaceholderSection', 'assembleSnapshot', 'parseUntrackedFiles', 'buildUnifiedGateDiff'];
  const parts = names.map((n) => extractFunctionSource(src, n));
  const consts = 'const PLACEHOLDER_SENTINELS = ["429", "rate limit", "placeholder"];\n';
  const bcr = extractFunctionSource(src, 'buildCommitRangeDiffInput');
  // eslint-disable-next-line no-new-func
  const factory = new Function(`${consts}${bcr}${parts.join('\n')}\nreturn buildUnifiedGateDiff;`);
  return factory();
}

const DRIFT_PAIRS = [
  { title: '(a) working-tree non-empty',
    input: { stat: ' a.js | 1 +', diff: 'diff --git a/a.js b/a.js', untracked: '',
      headPrevStat: ' b.js | 1 +', headPrevDiff: 'y', mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't1' } },
  { title: '(b) head-prev fallback',
    input: { stat: '', diff: '', untracked: '', headPrevStat: ' b.js | 2 +', headPrevDiff: 'y',
      mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't2' } },
  { title: '(c) merge-base fallback',
    input: { stat: '', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '',
      mergeBaseStat: ' c.js | 3 +', mergeBaseDiff: 'z', treeHash: null } },
  { title: '(d) untracked-only → intent-to-add',
    input: { stat: '', diff: '', untracked: '?? n.js\n?? m.js', headPrevStat: '', headPrevDiff: '',
      mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't3' } },
  { title: '(e) all empty → fail-closed',
    input: { stat: '', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '' } },
  { title: '429 placeholder exact match → fail-closed',
    input: { stat: '', diff: '429', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '' } },
  { title: 'substring "placeholder" in legit diff NOT fail-closed',
    input: { stat: ' p.js | 2 ++', diff: 'diff --git a/p.js b/p.js\n+const placeholder = 2;' } },
  { title: 'null input → fail-closed (no throw)', input: null },
];

test('drift guard: inlined buildUnifiedGateDiff matches canonical on all UC-1 pairs', () => {
  const inlined = loadInlinedUnified();
  for (const pair of DRIFT_PAIRS) {
    assert.deepEqual(inlined(pair.input), canonical(pair.input), `pair: ${pair.title}`);
  }
});

test('review-gate.js: inline copy carries exact-match placeholder sentinels (429/rate limit/placeholder)', () => {
  assert.ok(src.includes('PLACEHOLDER_SENTINELS'), 'sentinel constant inlined');
  for (const s of ['429', 'rate limit', 'placeholder']) {
    assert.ok(src.includes(`'${s}'`) || src.includes(`"${s}"`), `sentinel ${s} present`);
  }
});

test('review-gate.js: DIFF_FETCH_PROMPT collects merge-base range + untracked enumeration', () => {
  const start = src.indexOf('DIFF_FETCH_PROMPT = `');
  const end = src.indexOf('`;', start);
  assert.ok(start >= 0 && end > start, 'DIFF_FETCH_PROMPT template literal found');
  const body = src.slice(start, end);
  assert.ok(body.includes('git --no-pager diff HEAD~1..HEAD --stat'), 'head-prev stat');
  assert.ok(/merge-base/.test(body), 'merge-base command present');
  assert.ok(/origin\/master\.\.\.HEAD/.test(body), 'explicit merge-base range command');
  assert.ok(body.includes('git --no-pager status --porcelain'), 'untracked enumeration');
  assert.ok(body.includes('=== MERGE-BASE STAT ===') || body.includes('MERGE-BASE'), 'merge-base section header');
  assert.ok(body.includes('git write-tree'), 'tree hash command');
});

test('review-gate.js: wiring feeds buildUnifiedGateDiff with headPrev + mergeBase sections', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  const after = src.slice(fetchAt, fetchAt + 5000);
  assert.ok(after.includes('buildUnifiedGateDiff('), 'unified helper drives the decision');
  for (const f of ['headPrevStat', 'headPrevDiff', 'mergeBaseStat', 'mergeBaseDiff']) {
    assert.ok(after.includes(f), `field ${f} extracted`);
  }
  assert.ok(after.includes("'MERGE-BASE STAT'") || after.includes('MERGE-BASE STAT'), 'merge-base stat section extracted');
  assert.ok(after.includes("'MERGE-BASE DIFF'") || after.includes('MERGE-BASE DIFF'), 'merge-base diff section extracted');
});

test('review-gate.js: fail-closed (incl. 429-placeholder) short-circuits BEFORE reviewers', () => {
  const failAt = src.indexOf("gateDiff.mode === 'fail-closed'");
  assert.ok(failAt > 0, 'fail-closed branch on unified result');
  assert.ok(/NO-GO/.test(src.slice(failAt, failAt + 1500)), 'fail-closed judgment is NO-GO');
  for (const label of ['review:architecture', 'review:functional', 'review:maintainability']) {
    const at = src.indexOf(`label: '${label}'`);
    assert.ok(at > failAt, `${label} constructed after fail-closed check`);
  }
});

test('review-gate.js: intent-to-add mode injects untracked file list (no vacuous GO)', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  const after = src.slice(fetchAt, fetchAt + 6000);
  assert.ok(after.includes("'intent-to-add'"), 'intent-to-add mode branch exists');
  assert.ok(after.includes('untrackedFiles'), 'untracked file list used');
  const REVIEW_DIFF_RAW_at = src.indexOf('const REVIEW_DIFF_RAW');
  assert.ok(REVIEW_DIFF_RAW_at > 0, 'REVIEW_DIFF_RAW still built (Issue #91 T3 wiring kept)');
});
