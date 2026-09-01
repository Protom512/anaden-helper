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
  buildUnifiedGateDiff,
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

test('feature-pipeline: fail-closed uses unified T1 helper and short-circuits before reviewer prompts', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const after = fpSrc.slice(idx, idx + 6000);
  assert.ok(after.includes('buildUnifiedGateDiff('), 'wired to Issue #102 unified helper');
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

// ── Issue #102 T5: drift guard for the unified helper (inline copy in
//    feature-pipeline.js vs canonical review-gate-diff.js) — behavior comparison
//    over the full deterministic fallback chain (UC-1) ──

function extractInlineFunction(scriptSrc, name) {
  const start = scriptSrc.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `function ${name} must be inlined in feature-pipeline.js`);
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

function loadInlineUnifiedFromFp() {
  const parts = ['isPlaceholderSection', 'assembleSnapshot', 'parseUntrackedFiles', 'buildUnifiedGateDiff']
    .map((n) => extractInlineFunction(fpSrc, n));
  const bcr = extractInlineFunction(fpSrc, 'buildCommitRangeDiffInput');
  // eslint-disable-next-line no-new-func
  const factory = new Function(
    `const PLACEHOLDER_SENTINELS = ["429", "rate limit", "placeholder"];\n${bcr}${parts.join('\n')}\nreturn buildUnifiedGateDiff;`
  );
  return factory();
}

const UNIFIED_DRIFT_PAIRS = [
  { title: '(a) working-tree non-empty',
    input: { stat: ' a.js | 1 +', diff: 'diff --git a/a.js b/a.js', untracked: '',
      headPrevStat: ' b.js | 1 +', headPrevDiff: 'y', mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't1' } },
  { title: '(b) head-prev fallback',
    input: { stat: '', diff: '', untracked: '', headPrevStat: ' b.js | 2 +', headPrevDiff: 'y',
      mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't2' } },
  { title: '(c) merge-base fallback',
    input: { stat: '', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '',
      mergeBaseStat: ' c.js | 3 +', mergeBaseDiff: 'z', treeHash: null } },
  { title: '(d) untracked-only intent-to-add',
    input: { stat: '', diff: '', untracked: '?? n.js\n?? m.js', headPrevStat: '', headPrevDiff: '',
      mergeBaseStat: '', mergeBaseDiff: '', treeHash: 't3' } },
  { title: '(e) all empty fail-closed',
    input: { stat: '', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '' } },
  { title: '429 placeholder exact match fail-closed',
    input: { stat: '', diff: '429', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '' } },
  { title: 'rate limit placeholder in stat fail-closed',
    input: { stat: 'rate limit', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: '', mergeBaseDiff: '' } },
  { title: 'substring placeholder in legit diff NOT fail-closed',
    input: { stat: ' p.js | 2 ++', diff: 'diff --git a/p.js b/p.js\n+const placeholder = 2;' } },
  { title: 'merge-base placeholder fail-closed',
    input: { stat: '', diff: '', untracked: '', headPrevStat: '', headPrevDiff: '', mergeBaseStat: 'placeholder', mergeBaseDiff: '' } },
  { title: 'null input fail-closed no throw', input: null },
];

test('drift guard (Issue #102): inlined buildUnifiedGateDiff in feature-pipeline matches canonical', () => {
  const inlineUnified = loadInlineUnifiedFromFp();
  for (const pair of UNIFIED_DRIFT_PAIRS) {
    assert.deepEqual(inlineUnified(pair.input), buildUnifiedGateDiff(pair.input),
      `pair: ${pair.title}`);
  }
});

// ── Issue #102 T5 (ticket-precheck-wiring pattern): static verification of
//    gate-diff.json persistence + fail-closed pre-fanout short-circuit + TDZ
//    ordering of the persistence builder ──

test('wiring (Issue #102): fetch-diff prompt collects merge-base as a separate section', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const body = fpSrc.slice(fpSrc.lastIndexOf('agent(', idx), idx);
  assert.ok(/merge-base origin\/master HEAD/.test(body), 'merge-base command variant');
  assert.ok(body.includes('=== MERGE-BASE STAT ===') || body.includes('MERGE-BASE STAT'),
    'dedicated MERGE-BASE STAT section header');
  assert.ok(body.includes('MERGE-BASE DIFF'), 'dedicated MERGE-BASE DIFF section header');
});

test('wiring (Issue #102 T2): fetch-diff prompt enumerates untracked file contents explicitly', () => {
  const idx = fpSrc.indexOf("label: 'gate:fetch-diff'");
  const body = fpSrc.slice(fpSrc.lastIndexOf('agent(', idx), idx);
  // untracked-only fallback で reviewer が中身を読めるよう、ファイル内容列挙を明示指示
  assert.ok(/UNTRACKED FILE CONTENTS|untracked file contents/i.test(body),
    'explicit untracked file contents enumeration instruction');
  assert.ok(/head -c|cat\b/.test(body), 'content read command (head -c / cat) instructed');
});

test('wiring (Issue #102 T2): fetched untracked file contents reach the reviewer snapshot (intent-to-add mode)', () => {
  // UNTRACKED FILE CONTENTS sections must be extracted and joined into the
  // GATE_DIFF snapshot so reviewers see file bodies, not just names.
  const extractAt = fpSrc.indexOf('extractUntrackedFileContents');
  assert.ok(extractAt > 0, 'dedicated extractor for UNTRACKED FILE CONTENTS sections');
  const declAt = fpSrc.indexOf('const GATE_DIFF = [');
  const block = fpSrc.slice(declAt, declAt + 2000);
  assert.ok(/[Uu]ntrackedFileContents|UNTRACKED FILE CONTENTS/.test(block),
    'GATE_DIFF snapshot carries the untracked file contents sections');
});

test('wiring (Issue #102): gate-diff.json persistence agent exists with basis/treeHash/snapshot', () => {
  const labelAt = fpSrc.indexOf("label: 'gate:persist-diff'");
  assert.ok(labelAt > 0, 'persistence agent has a dedicated label (gate:persist-diff)');
  // builder definition sits ~14k chars upstream of the persist label (TDZ-safe
  // definition region), so the assertion window must span both.
  const around = fpSrc.slice(labelAt - 15500, labelAt + 200);
  assert.ok(around.includes('gate-diff.json'), 'gate-diff.json persistence file name present');
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.match(around, /\$\{runId\}/, 'path uses shared ${runId}');
  for (const f of ['mode', 'basis', 'treeHash', 'snapshot', 'recordedAt', 'snapshotChars', 'untrackedFiles']) {
    assert.ok(around.includes(f), `field ${f} persisted`);
  }
  // recordedAt は runTimestamp 流用 (ticket-precheck.json と同一パターン)
  assert.ok(fpSrc.includes('recordedAt: runTimestamp'),
    'recordedAt reuses runTimestamp (single source, ticket-precheck pattern)');
  // fail-closed path must carry failClosedReason (mode/basis mandatory even on failure)
  const failIdx = fpSrc.indexOf("label: 'gate:persist-diff-failed'");
  const failBlock = fpSrc.slice(failIdx - 2200, failIdx);
  assert.ok(failBlock.includes('failClosedReason'), 'fail-closed persistence carries failClosedReason');
  assert.ok(failBlock.includes('recordedAt'), 'fail-closed persistence carries recordedAt');
  assert.ok(failBlock.includes("'unknown'"), 'missing string values default to unknown (fail-closed convention)');
});

test('wiring (Issue #102): fail-closed short-circuit precedes all reviewer fan-out (pre-fanout, fail-closed)', () => {
  const failAt = fpSrc.indexOf("gateDiffInput.mode === 'fail-closed'");
  assert.ok(failAt > 0, 'fail-closed branch present');
  const persistAt = fpSrc.indexOf('gate:persist-diff-kind-rationale');
  const teamGateAt = fpSrc.indexOf('[gate-team-begin]');
  const instrAt = fpSrc.indexOf('FEEDBACK_INSTRUCTION');
  assert.ok(failAt < instrAt, 'fail-closed precedes FEEDBACK_INSTRUCTION build');
  assert.ok(failAt < persistAt, 'fail-closed precedes any persistence/fan-out agent');
  assert.ok(failAt < teamGateAt, 'fail-closed precedes gate team fan-out');
  // the fail-closed branch itself must return (resumable status)
  const failBlock = fpSrc.slice(failAt, failAt + 3000);
  assert.ok(failBlock.includes('GATE_DIFF_EMPTY_STATUS'), 'resumable empty status');
  assert.ok(/return\s*\{/.test(failBlock), 'fail-closed returns before fan-out');
  // 429 placeholder must route into the same fail-closed branch (no separate bypass)
  assert.ok(fpSrc.includes('429-placeholder'), '429-placeholder basis flows through fail-closed');
});

test('wiring (Issue #102): TDZ ordering — GATE_DIFF_SNAPSHOT builder executed only after ACTIVE_GATE_DIMENSIONS', () => {
  // estimate approval condition #3: the persistence builder must be invoked
  // strictly AFTER ACTIVE_GATE_DIMENSIONS is confirmed (past TDZ incident
  // integrationSliceCheck). Builder DEFINITION may precede; INVOCATION may not.
  const dimsAt = fpSrc.indexOf('const ACTIVE_GATE_DIMENSIONS =');
  const builderDefAt = fpSrc.indexOf('const buildGateDiffEvidence = () =>');
  const builderCallAt = fpSrc.indexOf('const gateDiffEvidence = buildGateDiffEvidence()');
  assert.ok(dimsAt > 0, 'ACTIVE_GATE_DIMENSIONS declaration found');
  assert.ok(builderDefAt > 0, 'buildGateDiffEvidence builder defined');
  assert.ok(builderCallAt > 0, 'buildGateDiffEvidence builder invoked');
  assert.ok(builderCallAt > dimsAt,
    `builder invocation (offset ${builderCallAt}) must come after ACTIVE_GATE_DIMENSIONS (offset ${dimsAt}) — TDZ guard`);
  const persistApply = fpSrc.indexOf('[gate-diff-kind-persist-apply]');
  assert.ok(builderCallAt > persistApply, 'invocation lives in the persist-apply (post-TDZ) region');
});

test('wiring (Issue #102): shared snapshot injected into every reviewer lane (single source)', () => {
  const gateDiffDecl = fpSrc.indexOf('const GATE_DIFF = [');
  assert.ok(gateDiffDecl > 0, 'GATE_DIFF snapshot array exists');
  const instrAt = fpSrc.indexOf('const FEEDBACK_INSTRUCTION');
  assert.ok(instrAt > gateDiffDecl, 'FEEDBACK_INSTRUCTION (carrier of GATE_DIFF) built after snapshot');
  const instrBlock = fpSrc.slice(instrAt, instrAt + 1200);
  assert.ok(instrBlock.includes('${GATE_DIFF}'), 'GATE_DIFF injected into the shared instruction');
  // persistence records the same GATE_DIFF (single source of truth)
  const persistIdx = fpSrc.indexOf('snapshot: GATE_DIFF');
  assert.ok(persistIdx > 0, 'persisted snapshot is the same GATE_DIFF string');
  // fail-closed path also persists its rationale to gate-diff.json
  const failPersistIdx = fpSrc.indexOf("label: 'gate:persist-diff-failed'");
  assert.ok(failPersistIdx > 0, 'fail-closed path persists to gate-diff.json too');
  const failAround = fpSrc.slice(failPersistIdx - 2500, failPersistIdx);
  assert.ok(failAround.includes('gate-diff.json') && failAround.includes('failClosed: true'),
    'fail-closed persistence records failClosed:true');
});
