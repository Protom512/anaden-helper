// TDD tests for Issue #78 Task 4: review-gate Analyze phase must inject the
// working-tree diff (R8 pattern ported from feature-pipeline.js) into all
// reviewer prompts, so findings are guaranteed to originate from the actual diff.
//
// Two layers:
//  - unit tests of the pure helpers in review-gate-diff.js (cap semantics)
//  - source-scan of review-gate.js (the workflow script itself is not importable:
//    top-level phase()/agent() calls)
//
// Run: node --test .claude/workflows/tests/review-gate-diff-inject.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  DIFF_TRUNCATE_THRESHOLD,
  DIFF_BODY_CAP,
  TOTAL_DIFF_CAP,
  DIFF_FETCH_PROMPT,
  normalizeGateDiff,
  buildDiffSection,
  withDiffContext,
} from '../review-gate-diff.js';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'review-gate.js');
const src = readFileSync(wfPath, 'utf8');

// ── cap constants consistent with feature-pipeline.js (approver condition) ──

test('cap constants match feature-pipeline R8 values (28000/24000/30000)', () => {
  assert.equal(DIFF_TRUNCATE_THRESHOLD, 28000);
  assert.equal(DIFF_BODY_CAP, 24000);
  assert.equal(TOTAL_DIFF_CAP, 30000);
  const fpSrc = readFileSync(join(dirname(wfPath), 'feature-pipeline.js'), 'utf8');
  assert.ok(fpSrc.includes('28000'), 'feature-pipeline 28000');
  assert.ok(fpSrc.includes('24000'), 'feature-pipeline 24000');
  assert.ok(DIFF_FETCH_PROMPT.includes(String(DIFF_TRUNCATE_THRESHOLD)), 'threshold interpolated into fetch prompt');
  assert.ok(DIFF_FETCH_PROMPT.includes(String(DIFF_BODY_CAP)), 'body cap interpolated into fetch prompt');
});

test('fetch prompt uses R8 commands: stat / full diff / status --porcelain + [DIFF TRUNCATED]', () => {
  assert.ok(DIFF_FETCH_PROMPT.includes('git --no-pager diff HEAD --stat'));
  assert.ok(DIFF_FETCH_PROMPT.includes('git --no-pager diff HEAD'));
  assert.ok(DIFF_FETCH_PROMPT.includes('git --no-pager status --porcelain'));
  assert.ok(DIFF_FETCH_PROMPT.includes('[DIFF TRUNCATED]'));
});

// ── (b) 30000 char total cap works (unit) ──

test('normalizeGateDiff caps at 30000 chars (b)', () => {
  const big = 'x'.repeat(TOTAL_DIFF_CAP + 5000);
  const capped = normalizeGateDiff(big);
  assert.equal(capped.length, 30000);
  assert.equal(capped, big.slice(0, 30000));
});

test('normalizeGateDiff: small strings unchanged; null -> ""; objects JSON-stringified then capped', () => {
  assert.equal(normalizeGateDiff('abc'), 'abc');
  assert.equal(normalizeGateDiff(null), '');
  assert.equal(normalizeGateDiff(undefined), '');
  const obj = { foo: 'bar' };
  assert.equal(normalizeGateDiff(obj), JSON.stringify(obj));
  const hugeObj = { data: 'y'.repeat(40000) };
  assert.equal(normalizeGateDiff(hugeObj).length, 30000);
});

// ── (a) reviewer prompt contains === WORKING-TREE DIFF === section (unit) ──

test('withDiffContext wraps prompt with WORKING-TREE DIFF section (a)', () => {
  const section = buildDiffSection('diff --git a/x b/x');
  const prompt = withDiffContext('Review this.', section);
  assert.ok(prompt.startsWith('Review this.'), 'base prompt preserved');
  assert.ok(prompt.includes('=== WORKING-TREE DIFF ==='));
  assert.ok(prompt.includes('diff --git a/x b/x'), 'diff body interpolated');
  assert.ok(prompt.includes('=== END DIFF ==='));
  assert.ok(prompt.indexOf('=== WORKING-TREE DIFF ===') < prompt.indexOf('=== END DIFF ==='), 'delimiters ordered');
});

test('empty diff: buildDiffSection/withDiffContext are no-ops (fail-open)', () => {
  assert.equal(buildDiffSection(''), '');
  assert.equal(withDiffContext('P', ''), 'P');
});

// ── wiring inside review-gate.js (source scan) ──

test('review-gate.js: diff fetched in Analyze BEFORE any reviewer prompt is built', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  assert.ok(fetchAt >= 0, 'diff fetch agent exists');
  assert.ok(src.slice(0, fetchAt).includes("phase('Analyze')"), 'fetch runs in Analyze phase');
  for (const label of ['review:architecture', 'review:functional', 'review:maintainability']) {
    const at = src.indexOf(`label: '${label}'`);
    assert.ok(at > fetchAt, `${label} constructed after diff fetch (no TDZ)`);
  }
});

test('review-gate.js: all 3 reviewer lanes pass REVIEW_DIFF_SECTION via withDiffContext', () => {
  assert.ok(src.includes('const REVIEW_DIFF_SECTION = buildDiffSection(REVIEW_DIFF_RAW)'),
    'REVIEW_DIFF_SECTION built from the T1-arbitrated raw diff (Issue #91 T3)');
  const uses = (src.match(/REVIEW_DIFF_SECTION\)/g) || []).length;
  assert.equal(uses, 3, `all 3 reviewer prompts wrap with withDiffContext (found ${uses})`);
});

test('review-gate.js: self-contained (no ESM import — Workflow runtime rejects imports, Issue #78 Task 5 live finding)', () => {
  assert.ok(!/^import /m.test(src), 'review-gate.js must not use ESM imports (scriptPath launchability)');
  assert.match(src, /Read して再取得\*しない\*/);
  assert.match(src, /hunk に基づく/);
});

test('review-gate.js: inlined cap constants match review-gate-diff.js (drift guard)', () => {
  const helperSrc = readFileSync(join(dirname(wfPath), 'review-gate-diff.js'), 'utf8');
  for (const cap of ['28000', '24000', '30000']) {
    assert.ok(src.includes(cap), `review-gate.js inlines ${cap}`);
    assert.ok(helperSrc.includes(cap), `review-gate-diff.js keeps ${cap}`);
  }
});

// ── Issue #91 T3: commit-range fallback + fail-closed mirrored in review-gate.js ──
// review-gate.js is not importable (top-level phase()/agent()), so the T1
// helpers are INLINED. Drift guard (approver condition): compare input/output
// PAIRS of the inlined helper against the canonical review-gate-diff.js copy —
// not just source-text equality — so behavior drift on future feature additions
// is detected even if the text stays superficially similar.

import {
  buildCommitRangeDiffInput as canonicalBuildCommitRangeDiffInput,
  extractDiffSection as canonicalExtractDiffSection,
} from '../review-gate-diff.js';

/** Extract a top-level `function name(...) {...}` source from a script by brace matching. */
function extractFunctionSource(scriptSrc, name) {
  const start = scriptSrc.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `function ${name} must be inlined in review-gate.js`);
  // Start matching AFTER the parameter list's closing paren — the first '{'
  // may itself be a default-parameter object literal (e.g. `options = {}`).
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
      if (c === '\\') i += 1; // skip escaped char
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

function loadInlinedHelper(name) {
  const fnSrc = extractFunctionSource(src, name);
  // eslint-disable-next-line no-new-func
  const factory = new Function(`${fnSrc}\nreturn ${name};`);
  return factory();
}

const DRIFT_PAIRS = [
  {
    title: 'working-tree diff non-empty',
    input: { stat: ' a.js | 1 +\n', diff: 'diff --git a/a.js b/a.js\n+x', untracked: '', rangeStat: ' b.js | 2 +\n', rangeDiff: 'y', treeHash: 't1' },
  },
  {
    title: 'working-tree empty -> commit-range fallback',
    input: { stat: '', diff: '', untracked: '', rangeStat: ' b.js | 2 +\n', rangeDiff: 'y', treeHash: 't2' },
  },
  {
    title: 'both empty -> fail-closed',
    input: { stat: '', diff: '', untracked: '', rangeStat: '', rangeDiff: '', treeHash: 't3' },
  },
  {
    title: 'untracked-only -> fail-closed with note',
    input: { stat: '', diff: '', untracked: '?? n.js\n', rangeStat: '', rangeDiff: '', treeHash: 't4' },
  },
  {
    title: 'whitespace-only diff treated as empty',
    input: { stat: '  \n', diff: '\t ', untracked: '', rangeStat: '', rangeDiff: '' },
  },
];

for (const variant of ['head-prev', 'merge-base']) {
  test(`drift guard: inlined buildCommitRangeDiffInput matches canonical outputs (${variant}) on all pairs`, () => {
    const inlined = loadInlinedHelper('buildCommitRangeDiffInput');
    for (const pair of DRIFT_PAIRS) {
      assert.deepEqual(
        inlined(pair.input, { rangeVariant: variant }),
        canonicalBuildCommitRangeDiffInput(pair.input, { rangeVariant: variant }),
        `pair: ${pair.title}`
      );
    }
  });
}

test('drift guard: inlined extractDiffSection matches canonical outputs on header pairs', () => {
  const inlined = loadInlinedHelper('extractDiffSection');
  const raw = ['=== STAT ===', ' a/b | 2 +-', '=== DIFF ===', 'diff --git a/b b/b', '=== UNTRACKED ===', '?? n.ts', '=== TREE HASH ===', 'abc'].join('\n');
  for (const name of ['STAT', 'DIFF', 'UNTRACKED', 'TREE HASH']) {
    assert.equal(inlined(raw, name), canonicalExtractDiffSection(raw, name), `section ${name}`);
  }
  assert.equal(inlined('no headers', 'DIFF'), canonicalExtractDiffSection('no headers', 'DIFF'));
  assert.equal(inlined('', 'DIFF'), canonicalExtractDiffSection('', 'DIFF'));
});

test('review-gate.js: fetch-diff prompt collects commit-range diff + tree hash (Issue #91 T3)', () => {
  // The prompt is a template literal assigned to DIFF_FETCH_PROMPT before the fetch agent.
  const start = src.indexOf('DIFF_FETCH_PROMPT = `');
  const end = src.indexOf('`;', start);
  const body = src.slice(start, end);
  assert.ok(start >= 0 && end > start, 'DIFF_FETCH_PROMPT template literal found');
  assert.ok(body.includes('git --no-pager diff HEAD~1..HEAD --stat'), 'commit-range stat command');
  assert.ok(body.includes('git --no-pager diff HEAD~1..HEAD'), 'commit-range full diff command');
  assert.ok(/merge-base/.test(body), 'merge-base fallback for merged contexts');
  assert.ok(body.includes('git write-tree'), 'tree hash command');
  assert.ok(body.includes('=== COMMIT-RANGE STAT ==='), 'commit-range stat header');
  assert.ok(body.includes('=== COMMIT-RANGE DIFF ==='), 'commit-range diff header');
  assert.ok(body.includes('=== TREE HASH ==='), 'tree hash header');
});

test('review-gate.js: wiring uses extractDiffSection + unified T1 helper after fetch', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  const after = src.slice(fetchAt, fetchAt + 4000);
  assert.ok(after.includes('extractDiffSection'), 'sections extracted from raw fetch result');
  // Issue #102 T4: the wiring now drives the fallback via buildUnifiedGateDiff
  // (which reuses buildCommitRangeDiffInput internally as the lower routine).
  assert.ok(after.includes('buildUnifiedGateDiff('), 'T1 unified helper drives the fallback decision');
  for (const s of ['STAT', 'DIFF', 'UNTRACKED', 'COMMIT-RANGE STAT', 'COMMIT-RANGE DIFF', 'TREE HASH']) {
    assert.ok(after.includes(`'${s}'`), `section '${s}' extracted`);
  }
});

test('review-gate.js: fail-closed branch short-circuits BEFORE any reviewer prompt is built', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  const after = src.slice(fetchAt, fetchAt + 4000);
  assert.ok(after.includes("mode === 'fail-closed'"), 'fail-closed branch exists');
  assert.ok(/return\s*\{/.test(after), 'fail-closed returns (no empty-diff injection)');
  assert.ok(after.includes('NO-GO'), 'fail-closed judgment is NO-GO');
  const failAt = src.indexOf("mode === 'fail-closed'");
  for (const label of ['review:architecture', 'review:functional', 'review:maintainability']) {
    const at = src.indexOf(`label: '${label}'`);
    assert.ok(at > failAt, `${label} constructed after fail-closed check`);
  }
});

test('review-gate.js: commit-range fallback note present when working-tree diff is empty (no vacuous GO)', () => {
  const fetchAt = src.indexOf("label: 'review:fetch-diff'");
  const after = src.slice(fetchAt, fetchAt + 4000);
  assert.ok(after.includes('commit-range'), 'commit-range fallback mentioned in wiring comments/log');
  assert.ok(!after.includes('normalizeGateDiff(diffFetch)'), 'raw fetch no longer injected blind (T1 helper arbitrates)');
});
