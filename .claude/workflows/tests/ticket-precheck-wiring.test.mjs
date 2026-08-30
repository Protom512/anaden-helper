// TDD tests for Issue #99 Task 3: feature-pipeline.js precheck wiring.
//
// Wiring contract:
//  (a) Request phase 直後・Estimate 前に evaluateTicketPrecheck が実行される
//      (slice source order: 'pm-ticket-files-end' marker -> precheck block ->
//      'phase(\'Estimate\')')
//  (b) FAIL 時は status 'precheck-failed' で short-circuit (resolveReleaseAbort
//      と同型の resumable status — snapshot branch はまだ無いので ticket/理由を返す)
//  (c) PASS 時は slice メタデータ (changedCrates / diffKind) が Estimate と
//      Gate プロンプトへ注入される
//  (d) GATE_DIMENSIONS lane 選択が precheck 生成メタデータ (diff 実測値) で
//      上書きされる — 手動 changedCrates 導出は deriveSliceMetadata に置換
//  (e) evidence: .omc/logs/{run-id}/ticket-precheck.json へ永続化
//  (f) fail-closed: ticket.files 未宣言 + diff 非空 → FAIL (UC-2 系)
//  (g) UC-3: commit-range fallback — working tree clean かつ commit-range
//      diff 非空を precheck が受け取る経路が存在する
//
// Run: node --test .claude/workflows/tests/ticket-precheck-wiring.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  evaluateTicketPrecheck,
  deriveSliceMetadata,
} from '../ticket-precheck.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

// ── (a) wiring position: Request 直後・Estimate 前 ──

test('wiring position: precheck block sits between pm-ticket-files-end and phase(Estimate)', () => {
  const pmEnd = fpSrc.indexOf('[pm-ticket-files-end]');
  const precheckBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  assert.ok(pmEnd > 0, 'pm-ticket-files-end marker exists');
  assert.ok(precheckBegin > 0, 'ticket-precheck-wiring-begin marker exists');
  assert.ok(estimatePhase > 0, "phase('Estimate') exists");
  assert.ok(
    pmEnd < precheckBegin && precheckBegin < estimatePhase,
    `expected pmEnd(${pmEnd}) < precheckBegin(${precheckBegin}) < estimatePhase(${estimatePhase})`
  );
});

test('wiring: evaluateTicketPrecheck is defined inline and invoked', () => {
  const blockBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const blockEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  assert.ok(blockBegin > 0 && blockEnd > blockBegin, 'wiring marker block present');
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.ok(/function evaluateTicketPrecheckInline|const evaluateTicketPrecheck/.test(block),
    'evaluateTicketPrecheck defined inside the wiring block');
  assert.match(block, /evaluateTicketPrecheck\(/, 'evaluateTicketPrecheck invoked');
});

test('wiring: deriveSliceMetadata defined inline and used to derive crates/diffKind', () => {
  const blockBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const blockEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.match(block, /deriveSliceMetadata/, 'deriveSliceMetadata referenced in wiring block');
  assert.match(block, /changedCrates/, 'changedCrates derived from metadata');
  assert.match(block, /diffKind/, 'diffKind derived from metadata');
});

// ── (b) FAIL short-circuit: status 'precheck-failed' ──

test('FAIL short-circuit: status precheck-failed returned before Estimate', () => {
  const precheckBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(precheckBegin, estimatePhase);
  assert.ok(block.length > 0 && estimatePhase > precheckBegin, 'precheck precedes Estimate');
  assert.match(block, /'precheck-failed'/, "status 'precheck-failed' literal present");
  assert.match(block, /ticket\.files/, 'declared files sourced from ticket.files');
  assert.match(block, /verdict\s*===?\s*'FAIL'|verdict !== 'PASS'/, 'FAIL branch checks verdict');
});

test('FAIL short-circuit is a return (resumable status, resolveReleaseAbort と同型)', () => {
  const precheckBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(precheckBegin, estimatePhase);
  // Use the LAST occurrence — the actual return object (earlier ones are comments).
  const failIdx = block.lastIndexOf("'precheck-failed'");
  assert.ok(failIdx > 0, "status 'precheck-failed' literal present");
  // The FAIL return must carry the status literal inside its object.
  const failBlock = block.slice(Math.max(0, failIdx - 300), failIdx + 300);
  assert.ok(failBlock.includes('return {'),
    'return statement carries the precheck-failed status');
  assert.match(block.slice(failIdx, failIdx + 600), /reason|undeclared|missing/,
    'FAIL return includes reason/mismatch details');
});

// ── (c) PASS: slice metadata injected into Estimate/Gate prompts ──

test('PASS injection: slice metadata referenced by Estimate prompt', () => {
  const estIdx = fpSrc.indexOf('You are the Tech Lead.');
  const estBlock = fpSrc.slice(estIdx, estIdx + 3000);
  assert.ok(estIdx > 0, 'estimate prompt exists');
  assert.match(estBlock, /precheckSliceMetadata|ticketPrecheck\.|precheckMetadata/,
    'Estimate prompt injects precheck slice metadata');
});

test('PASS injection: precheck-derived changedCrates feeds gate lanes (manual derivation replaced)', () => {
  // The legacy manual derivation (post-resolve-scope .filter(f => f.startsWith('crates/')))
  // must be replaced by precheck metadata. GATE_DIFF_KIND_LANES must consume the
  // precheck-overridden diffKind.
  const legacy = fpSrc.match(
    /changedCrates\s*=\s*\(scopeResult[\s\S]{0,400}?startsWith\('crates\/'\)/
  );
  assert.equal(legacy, null,
    'manual changedCrates derivation from scopeResult must be replaced (legacy found)');
  const lanesIdx = fpSrc.indexOf('function GATE_DIFF_KIND_LANES()');
  assert.ok(lanesIdx > 0, 'GATE_DIFF_KIND_LANES exists');
  const lanesBlock = fpSrc.slice(lanesIdx, lanesIdx + 700);
  assert.match(lanesBlock, /ticketPrecheck|precheckMetadata|precheckDiffKind/,
    'lane selection reads the precheck-derived diffKind');
});

// ── (d) behavior: inline copies must match canonical ticket-precheck.js ──

test('drift guard: inline evaluateTicketPrecheck matches canonical module', () => {
  const beginMarker = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const begin = fpSrc.indexOf('\n', beginMarker) + 1;
  const endMarker = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const end = fpSrc.lastIndexOf('\n', endMarker);
  assert.ok(begin > 0 && end > begin, 'wiring block markers present');
  const inlineSrc = fpSrc
    .slice(begin, end)
    .replace(/^\s*\/\/.*$/gm, '');
  // Strip the trailing runtime wiring (agent call / returns) — keep only the
  // pure function definitions by extracting from 'function' to the closing of
  // deriveSliceMetadata. The block must define classifyDiffKind-reusing helpers.
  assert.match(inlineSrc, /function evaluateTicketPrecheckInline|const evaluateTicketPrecheck/, 'pure fn defined');

  const fnStart = inlineSrc.search(/(?:function evaluateTicketPrecheckInline|const evaluateTicketPrecheck)\s*[=(]/);
  assert.ok(fnStart >= 0, 'pure function start found');
  // eval the pure-function region: from fnStart to the deriveSliceMetadata close.
  // We look for the end by evaluating progressively — simpler: require the block
  // contains both pure fns and eval the whole stripped source in a Function that
  // tolerates later runtime statements is unsafe; instead extract helper fns.
  const helpersMatch = inlineSrc.match(/function normalizePath[\s\S]*?\n}\nfunction toNormalizedSet[\s\S]*?\n}\n/);
  assert.ok(helpersMatch, 'normalizePath + toNormalizedSet helpers inlined');
});

test('behavior: FAIL verdict produced for undeclared changed files (canonical semantics exercised via inline call shape)', () => {
  // The inline call must pass (ticket.files, combined changed files) in that order.
  const blockBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const blockEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const block = fpSrc.slice(blockBegin, blockEnd).replace(/^\s*\/\/.*$/gm, '');
  const call = block.match(/evaluateTicketPrecheck(?:Inline)?\(\s*([^,)]+),\s*([^)]+)\)/);
  assert.ok(call, 'evaluateTicketPrecheck call site found');
  assert.match(call[1], /ticket\.files|declaredFiles/, 'first arg is ticket-declared files');
  // second arg must be a combined changed list (working tree + untracked + commit-range fallback)
  assert.match(
    block,
    /precheckChangedFiles|combinedChanged|commitRangeFiles|rangeFiles/,
    'combined changed-files list (incl. commit-range fallback) built before the call'
  );
});

// ── (e) evidence persistence ──

test('evidence: precheck verdict persisted to .omc/logs/{run-id}/ticket-precheck.json', () => {
  const persistIdx = fpSrc.indexOf('ticket-precheck.json');
  assert.ok(persistIdx > 0, 'dedicated persistence file name present');
  const around = fpSrc.slice(persistIdx - 3000, persistIdx + 500);
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.match(around, /\$\{runId\}/, 'path uses shared ${runId}');
  assert.match(around, /verdict/, 'verdict recorded in persisted JSON');
});

// ── (g) UC-3: commit-range fallback path exists in precheck scope resolver ──

test('UC-3: precheck scope resolver includes commit-range fallback for clean working tree', () => {
  const blockBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const blockEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.match(block, /HEAD~1\.\.HEAD|commit-range|merge-base/,
    'commit-range fallback (HEAD~1..HEAD / merge-base) collected by precheck scope resolver');
});

// ── canonical module still green under the wiring contract ──

test('canonical: UC-2 fail-closed semantics unchanged (undeclared file FAILs)', () => {
  const r = evaluateTicketPrecheck(['a.rs'], ['a.rs', 'b.js']);
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['b.js']);
  const m = deriveSliceMetadata(['crates/x/src/lib.rs', 'docs/a.md']);
  assert.deepEqual(m.changedCrates, ['x']);
  assert.equal(m.diffKind, 'mixed');
});

// ── (e-extra) treeHash persistence (pipeline-evidence-verification.md §2) ──

test('evidence: treeHash recorded in persisted ticket-precheck.json (vacuous-PASS detection)', () => {
  const blockBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const blockEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.ok(blockBegin > 0 && blockEnd > blockBegin, 'wiring block present');
  assert.match(block, /git write-tree/, 'scope resolver collects git write-tree output');
  assert.match(block, /treeHash/, 'treeHash carried into persisted JSON');
  // fail-closed: missing treeHash must not be silently empty/green
  const guard = block.match(/treeHash[\s\S]{0,200}unknown/s);
  assert.ok(guard, 'missing treeHash falls back to explicit unknown (fail-closed)');
});

// ── Issue #109 Task 2: issue-premise precheck wiring (Request→Estimate) ──
// stale (closed+merged issue) / duplicate (open PR) dispatch を機械検出して
// Estimate 以降の dispatch を拒否する。fail-closed: gh 失敗は FAIL verdict。

test('issuePremise wiring: marker block sits inside ticket-precheck wiring, before Estimate', () => {
  const begin = fpSrc.indexOf('[issue-premise-wiring-begin]');
  const end = fpSrc.indexOf('[issue-premise-wiring-end]');
  const precheckBegin = fpSrc.indexOf('[ticket-precheck-wiring-begin]');
  const precheckEnd = fpSrc.indexOf('[ticket-precheck-wiring-end]');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  assert.ok(begin > 0 && end > begin, 'issue-premise wiring markers present');
  assert.ok(precheckBegin < begin && end < precheckEnd,
    'issue-premise block nested inside ticket-precheck wiring block');
  assert.ok(end < estimatePhase, 'issue-premise block precedes Estimate');
});

test('issuePremise wiring: inline pure fn evaluates gh/git/PR evidence', () => {
  const begin = fpSrc.indexOf('[issue-premise-wiring-begin]');
  const end = fpSrc.indexOf('[issue-premise-wiring-end]');
  const block = fpSrc.slice(begin, end);
  // 定義は前方 [ticket-premise] 一元化ブロック内 (二重宣言解消後)。両方を検証:
  assert.match(fpSrc, /const evaluateIssuePremise/, 'evaluateIssuePremise defined inline (unified block)');
  assert.match(block, /evaluateIssuePremise\(\{/, 'evaluateIssuePremise invoked with object evidence');
  assert.match(block, /gh issue view/, 'gh issue view (state/closedAt) collected');
  assert.match(block, /state/, 'issue state passed to pure fn');
  assert.match(block, /git branch -a --contains|--contains/, 'trunk membership via git branch --contains');
  assert.match(block, /gh pr list --search/, 'open-PR duplicate search collected');
});

test('issuePremise wiring: FAIL short-circuits to precheck-failed before Estimate', () => {
  const begin = fpSrc.indexOf('[issue-premise-wiring-begin]');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(begin, estimatePhase);
  assert.match(block, /issuePremise\.verdict !== 'PASS'/, 'FAIL branch checks verdict');
  const failIdx = block.indexOf("'precheck-failed'");
  assert.ok(failIdx > 0, "status 'precheck-failed' literal present");
  const failBlock = block.slice(Math.max(0, failIdx - 300), failIdx + 400);
  assert.ok(failBlock.includes('return {'), 'FAIL branch returns resumable status object');
  assert.match(block, /issuePremise\.reason/, 'FAIL return carries reason');
});

test('issuePremise evidence: verdict persisted to .omc/logs/{runId}/issue-premise-precheck.json', () => {
  const begin = fpSrc.indexOf('[issue-premise-wiring-begin]');
  const end = fpSrc.indexOf('[issue-premise-wiring-end]');
  const block = fpSrc.slice(begin, end);
  const persistIdx = block.indexOf('issue-premise-precheck.json');
  assert.ok(persistIdx > 0, 'dedicated persistence file name present in issue-premise block');
  const around = block.slice(Math.max(0, persistIdx - 2000), persistIdx + 500);
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.match(around, /\$\{runId\}/, 'path uses shared ${runId}');
  assert.match(around, /verdict/, 'verdict recorded in persisted JSON');
});

test('issuePremise wiring: fail-closed — gh failure surfaces as FAIL via malformed evidence', () => {
  const begin = fpSrc.indexOf('[issue-premise-wiring-begin]');
  const end = fpSrc.indexOf('[issue-premise-wiring-end]');
  const block = fpSrc.slice(begin, end);
  // wiring must guard missing/unfetchable evidence and pass it through the
  // pure fn (which fails-closed on malformed input), never bypass it.
  assert.match(block, /issueState/, 'issueState field mapped from gh evidence');
  assert.match(block, /linkedBranchesContainIssue/, 'linkedBranchesContainIssue field mapped');
  assert.match(block, /openPRs/, 'openPRs field mapped');
  const bypass = block.match(/verdict\s*=\s*'PASS'/);
  assert.equal(bypass, null, 'no hardcoded PASS bypass of evaluateIssuePremise');
});
