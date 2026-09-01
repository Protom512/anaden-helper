// TDD tests for Issue #95 (P-008) T4b/T4c: feature-pipeline.js wiring.
//
// Layers (same structure as tests/gate-commit-range-diff.test.mjs):
//  (b) drift guard — inlined diff-kind helpers in feature-pipeline.js must
//      behave identically to the canonical gate-diff-kind.js module
//      (ESM imports are rejected by the Workflow runtime, so the pipeline
//      script inlines a copy; tests compare input/output pairs against the
//      canonical module, not just source text)
//  (c) short-circuit lane accounting — with a docs-only diff the expected
//      lane count must be derived from the active lane set (never a
//      hard-coded 6), and the aggregate must not report MISSING for lanes
//      that were intentionally short-circuited
//  (extra) dead-path removal — the legacy SR-3 `.md`-only isDocsOnly const
//      (L171-177 pre-T3) must be gone (approver condition on T3)
//
// Run: node --test .claude/workflows/tests/gate-diff-kind-wiring.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyDiffKind, isDocsPath } from '../gate-diff-kind.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

// ── (b) drift guard: inline copy vs canonical module ──

test('drift guard: feature-pipeline contains an inline gate-diff-kind marker block', () => {
  assert.ok(
    fpSrc.includes('gate-diff-kind'),
    'feature-pipeline.js must reference gate-diff-kind (inline copy + drift-guard marker)'
  );
});

test('drift guard: inlined isDocsPath/classifyDiffKind match canonical behavior', () => {
  const beginMarker = fpSrc.indexOf('gate-diff-kind-begin');
  const end = fpSrc.indexOf('gate-diff-kind-end');
  // Slice from AFTER the begin-marker line (markers are comment lines) to BEFORE the end-marker line.
  const begin = fpSrc.indexOf('\n', beginMarker) + 1;
  const endCut = fpSrc.lastIndexOf('\n', end);
  assert.ok(begin > 0 && end > begin, 'inline gate-diff-kind block markers present');
  const inlineSrc = fpSrc.slice(begin, endCut);
  const inline = new Function(
    `${inlineSrc.replace(/^\s*\/\/.*$/gm, '')}\nreturn { isDocsPath, classifyDiffKind };`
  )();

  const pathCases = [
    'a/b.md', 'README.md', 'a/b.markdown', 'A/B.MD',
    'docs/anything.txt', 'doc/x', '.claude/rules/r.md', '.claude/rules/sub/x.txt',
    'src/main.rs', 'Cargo.toml', 'crates/foo/README.md',
    'a\\b\\c.md', '', '   ', null, undefined, 42, [], {},
  ];
  for (const p of pathCases) {
    assert.equal(
      inline.isDocsPath(p),
      isDocsPath(p),
      `isDocsPath(${JSON.stringify(String(p))}) diverged from canonical`
    );
  }

  const listCases = [
    ['README.md', 'docs/guide.md'],
    ['.claude/rules/new-rule.md', 'CLAUDE.md'],
    ['docs/adr/0001.txt', 'README.md'],
    ['crates/foo/src/lib.rs'],
    ['Cargo.toml'],
    ['README.md', 'crates/foo/src/lib.rs'],
    ['docs/a.md', '.claude/workflows/feature-pipeline.js'],
    [],
    null,
    undefined,
    'README.md',
    ['', '   '],
    ['README.md', null, 42],
    ['data/schema.json'],
    ['image.png'],
    ['a\\b\\c.md'],
  ];
  for (const c of listCases) {
    assert.equal(
      inline.classifyDiffKind(c),
      classifyDiffKind(c),
      `classifyDiffKind(${JSON.stringify(c)}) diverged from canonical`
    );
  }
});

test('drift guard: inline copy is actually wired (classifyDiffKind called near the gate)', () => {
  const callCount = fpSrc.split('classifyDiffKind(').length - 1;
  assert.ok(callCount >= 2, `classifyDiffKind must be invoked (found ${callCount} occurrences incl. definition)`);
});

// ── (c) short-circuit lane accounting ──

test('short-circuit: expected lane count resolved dynamically, not hard-coded GATE_DIMENSIONS.length', () => {
  // Both the polling loop and the aggregate must use the active lane count
  // (docs-only short-circuit removes lanes; a stale 6 would mark them MISSING
  // and false-fire gate-incomplete).
  const aggIdx = fpSrc.indexOf('aggregateGateReviews(gateReviews');
  assert.ok(aggIdx > 0, 'unified aggregate call exists');
  const aggCall = fpSrc.slice(aggIdx, fpSrc.indexOf(')', aggIdx));
  assert.ok(
    !/(^|[^A-Za-z_])GATE_DIMENSIONS\.length/.test(aggCall),
    `unified aggregate must not use the full-dimension GATE_DIMENSIONS.length (found: ${aggCall.trim()})`
  );
  assert.ok(
    /ACTIVE_GATE_DIMENSIONS\.length|activeLaneCount/.test(aggCall),
    `unified aggregate must use the active lane count (found: ${aggCall.trim()})`
  );

  // polling loop: `new Array(GATE_DIMENSIONS.length).fill(null)` must also
  // resolve dynamically.
  const pollIdx = fpSrc.indexOf('new Array(');
  assert.ok(pollIdx > 0, 'polling loop allocation exists');
  const alloc = fpSrc.slice(pollIdx, fpSrc.indexOf(')', pollIdx));
  assert.ok(
    !/(^|[^A-Za-z_])GATE_DIMENSIONS\.length/.test(alloc),
    `polling allocation must not hard-code GATE_DIMENSIONS.length (found: ${alloc})`
  );
});

test('short-circuit: aggregate treats only truly-run lanes as expected (behavioral, canonical aggregateGateReviews contract)', () => {
  // Extract the inlined aggregateGateReviews + normalizeGateReview from
  // feature-pipeline.js and verify that when fewer lanes are expected
  // (docs-only short-circuit), complete collections are not INCOMPLETE.
  // Comments are stripped so the eval never sees stray identifiers, and the
  // block is bounded at the R6 fall-back close so no top-level `await` leaks in.
  const begin = fpSrc.indexOf('const normalizeGateReview');
  const end = fpSrc.indexOf('[gate-team-end]');
  assert.ok(begin > 0 && end > begin, 'aggregateGateReviews inline block found');
  const block = fpSrc.slice(begin, end).replace(/^\s*\/\/.*$/gm, '');
  const closing = '};\n';
  const lastBrace = block.lastIndexOf('};');
  assert.ok(lastBrace > 0, 'aggregateGateReviews definition closes');
  const inline = new Function(
    `${block.slice(0, lastBrace + closing.length)}\nreturn { aggregateGateReviews };`
  )();

  const lane = (verdict) => ({ verdict, dimension: 'x', findings: [], summary: '' });

  // 4/4 expected (2 lanes short-circuited away) => complete GO, no MISSING
  const agg4 = inline.aggregateGateReviews([lane('GO'), lane('GO'), lane('GO'), lane('GO')], 4);
  assert.equal(agg4.preVerdict, 'GO');
  assert.equal(agg4.complete, true);
  assert.deepEqual(agg4.missing, []);

  // null beyond expected count (never ran) must not flip a complete gate
  const agg4b = inline.aggregateGateReviews(
    [lane('GO'), lane('GO'), lane('GO'), lane('GO'), null, null], 4
  );
  assert.notEqual(agg4b.preVerdict, 'INCOMPLETE');
  assert.equal(agg4b.complete, true);

  // a missing lane INSIDE the expected set still blocks (fail-closed kept)
  const aggMissing = inline.aggregateGateReviews([lane('GO'), null, lane('GO'), lane('GO')], 4);
  assert.equal(aggMissing.preVerdict, 'INCOMPLETE');
  assert.equal(aggMissing.complete, false);

  // short-circuit with a NO-GO still NO-GO (never under-review)
  const aggNoGo = inline.aggregateGateReviews([lane('GO'), lane('NO-GO'), lane('GO'), lane('GO')], 4);
  assert.equal(aggNoGo.preVerdict, 'NO-GO');
});

test('short-circuit: docs-only keeps governance lane + at least one code-adjacent lane (never zero)', () => {
  // Approver condition: never collapse to zero lanes.
  const scIdx = fpSrc.indexOf("classifyDiffKind");
  assert.ok(scIdx > 0, 'classifyDiffKind wired');
  const after = fpSrc.slice(scIdx, scIdx + 6000);
  assert.ok(
    /docs-only|'docs-only'/.test(after),
    'docs-only short-circuit branch exists near the classifier call'
  );
  assert.ok(
    /filter\(/.test(after),
    'lane set is filtered (subset), not replaced wholesale'
  );
  assert.ok(
    !/GATE_DIMENSIONS\s*=\s*\[\s*\]/.test(fpSrc),
    'GATE_DIMENSIONS never emptied'
  );
});

// ── (extra) dead-path removal: legacy isDocsOnly const must be gone ──

test('legacy dead isDocsOnly (.md-only) const removed (T3 approver condition)', () => {
  const legacy = fpSrc.match(/const isDocsOnly\b/g);
  assert.equal(legacy, null, 'legacy isDocsOnly const must be removed, not left beside the new path');
  const endsWithMd = fpSrc.match(/every\(\(?[^)]*\)?\s*=>\s*\S*\.endsWith\('\.md'\)/g);
  assert.equal(endsWithMd, null, 'the `.md`-only every() check must not remain');
});

// ── (d) short-circuit rationale persistence (Issue #95 P-008 T3c) ──
// Evidence は自己申告不可 (.claude/rules/pipeline-evidence-verification.md):
// short-circuit 判定根拠 (classification / basis files / treeHash) は
// .omc/logs/{run-id}/ への永続ログとして残す。

test('T3c: short-circuit rationale persisted to .omc/logs/{run-id}/diff-kind-short-circuit.json', () => {
  const persistIdx = fpSrc.indexOf('diff-kind-short-circuit.json');
  assert.ok(persistIdx > 0, 'dedicated persistence log file name present');
  const around = fpSrc.slice(persistIdx - 3500, persistIdx + 500);
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.ok(/classification|diffKind/.test(around), 'classification recorded');
  assert.ok(/basisFiles|changedFilesArr/.test(around), 'basis files recorded');
  assert.ok(around.includes('treeHash'), 'treeHash recorded (tree identity of the classified diff)');
});

test('T3d: team-gate-protocol exposes resolveGateLanes for dynamic lane resolution', async () => {
  const tgp = await import('../team-gate-protocol.js');
  assert.equal(typeof tgp.resolveGateLanes, 'function', 'resolveGateLanes exported');
  const full = ['reliability', 'performance', 'extensibility', 'governance', 'security', 'integration'];
  assert.deepEqual(tgp.resolveGateLanes({ diffKind: 'code', dimensions: full }).lanes, full);
  const docs = tgp.resolveGateLanes({ diffKind: 'docs-only', dimensions: full });
  assert.ok(docs.shortCircuited === true);
  assert.ok(docs.lanes.includes('governance'), 'governance kept');
  assert.ok(docs.lanes.length >= 1 && docs.lanes.length <= 2, 'governance-centered 1-2 lanes');
  // unknown kind -> full lanes, fail-closed
  assert.equal(tgp.resolveGateLanes({ diffKind: 'bogus', dimensions: full }).lanes.length, 6);
});

test('T3d: aggregateGateVerdicts accepts explicit expectedDimensions (docs-only count)', async () => {
  const tgp = await import('../team-gate-protocol.js');
  const two = [
    { verdict: 'GO', dimension: 'governance', findings: [], summary: '' },
    { verdict: 'GO', dimension: 'integration', findings: [], summary: '' },
  ];
  const r = tgp.aggregateGateVerdicts(two, { expectedDimensions: 2 });
  assert.equal(r.preVerdict, 'GO');
  assert.equal(r.missing.length, 0);
});

// ── (e) Issue #97 Task 3: runTimestamp injection (traceability) ──
// 全永続 JSON (.omc/logs/{run-id}/ 配下) と reviewer prompt から runTimestamp
// が参照可能であること。runId は run 開始時の共有値 (diff-kind 用に個別生成しない)。

test('T3(#97): diffKindRationale records the shared run-level runId + runTimestamp', () => {
  // TDZ 修正後、rationale は builder (buildDiffKindRationale) 経由で構築される。
  // 実体のフィールド定義は builder 内にあるため、そちらを検証対象とする。
  const rIdx = fpSrc.indexOf('const buildDiffKindRationale');
  assert.ok(rIdx > 0, 'buildDiffKindRationale definition exists');
  const block = fpSrc.slice(rIdx, rIdx + 900);
  assert.ok(/runTimestamp/.test(block), 'runTimestamp field present in diffKindRationale builder');
  // builder は ACTIVE_GATE_DIMENSIONS 確定後に適用されること (TDZ-safe ordering)
  const applyIdx = fpSrc.indexOf('const diffKindRationale = buildDiffKindRationale()');
  const lanesIdx = fpSrc.indexOf('const ACTIVE_GATE_DIMENSIONS');
  assert.ok(applyIdx > lanesIdx, 'diffKindRationale built after ACTIVE_GATE_DIMENSIONS (TDZ-safe)');
  assert.ok(/runId/.test(block), 'runId field present in diffKindRationale');
  // runId must reference the shared run-level const, not a locally re-generated id
  assert.ok(!/diffKindRunId/.test(fpSrc), 'per-file diffKindRunId (separate run dir) must be removed — persist under the shared run dir');
  // persistence path must use the shared runId dir so all JSONs co-locate.
  // Search ALL occurrences (Issue #102 added the gate-diff builder above, which
  // can shift the window of the first hit — the actual persister prompt is the
  // occurrence prefixed with the .omc/logs/ path in an EVIDENCE PERSISTER agent).
  const occurrences = [...fpSrc.matchAll(/diff-kind-short-circuit\.json/g)].map((m) => m.index);
  assert.ok(occurrences.length > 0, 'diff-kind persistence log file name present');
  const usesSharedRunId = occurrences.some(
    (i) => fpSrc.slice(Math.max(0, i - 1500), i).includes('${runId}')
  );
  assert.ok(usesSharedRunId, 'diff-kind persistence writes into .omc/logs/${runId}/ (shared run dir)');
});

test('T3(#97): GATE_DIFF reviewer blob carries the run timestamp', () => {
  const gIdx = fpSrc.indexOf('const GATE_DIFF = [');
  assert.ok(gIdx > 0, 'GATE_DIFF array exists');
  const block = fpSrc.slice(gIdx, gIdx + 500);
  assert.ok(/RUN TIMESTAMP|runTimestamp/.test(block), 'GATE_DIFF includes a run timestamp line/section');
});

test('T3(#97): formatEvidenceForReviewers surfaces runTimestamp (fail-closed unknown)', () => {
  const fIdx = fpSrc.indexOf('const formatEvidenceForReviewers = (evidence');
  assert.ok(fIdx > 0, 'formatEvidenceForReviewers exists');
  const block = fpSrc.slice(fIdx, fpSrc.indexOf('};', fIdx));
  assert.ok(/runTimestamp/.test(block), 'runTimestamp parameter referenced in evidence blob');
  assert.ok(/unknown/.test(block), 'missing runTimestamp falls back to unknown (fail-closed)');
  assert.ok(
    fpSrc.includes('formatEvidenceForReviewers(gateEvidence, runTimestamp)'),
    'call site passes the run-level runTimestamp'
  );
});

test('T3(#97): release-review reviewer prompts (team + fallback) reference the run timestamp', () => {
  // both the team-path reviewerPrompt and the fallback parallel() prompt
  const teamPrompt = fpSrc.indexOf('const reviewerPrompt = (lens, i) =>');
  assert.ok(teamPrompt > 0, 'team-path reviewer prompt exists');
  const teamBlock = fpSrc.slice(teamPrompt, teamPrompt + 700);
  assert.ok(/runTimestamp/.test(teamBlock), 'team-path prompt carries runTimestamp');
  const fbIdx = fpSrc.indexOf('RELEASE REVIEWER ${i + 1}/3');
  assert.ok(fbIdx > 0, 'fallback reviewer prompt exists');
  const fbBlock = fpSrc.slice(fbIdx, fbIdx + 500);
  assert.ok(/runTimestamp/.test(fbBlock), 'fallback prompt carries runTimestamp');
});
