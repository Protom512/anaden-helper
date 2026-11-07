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

test('wiring position: precheck block sits between the PM ticket block and phase(Estimate)', () => {
  const pmEnd = fpSrc.indexOf('const runMeta = buildRunMetadata');
  const precheckBegin = fpSrc.indexOf('function normalizePath');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  assert.ok(pmEnd > 0, 'PM ticket block boundary (runMetadata persister) exists');
  assert.ok(precheckBegin > 0, 'ticket-precheck wiring region (normalizePath) exists');
  assert.ok(estimatePhase > 0, "phase('Estimate') exists");
  assert.ok(
    pmEnd < precheckBegin && precheckBegin < estimatePhase,
    `expected pmEnd(${pmEnd}) < precheckBegin(${precheckBegin}) < estimatePhase(${estimatePhase})`
  );
});

test('wiring: evaluateTicketPrecheck is defined inline and invoked', () => {
  const blockBegin = fpSrc.indexOf('function normalizePath');
  const blockEnd = fpSrc.indexOf("phase('Estimate')");
  assert.ok(blockBegin > 0 && blockEnd > blockBegin, 'wiring block present');
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.ok(/function evaluateTicketPrecheckInline|const evaluateTicketPrecheck/.test(block),
    'evaluateTicketPrecheck defined inside the wiring block');
  assert.match(block, /evaluateTicketPrecheck\(/, 'evaluateTicketPrecheck invoked');
});

test('wiring: deriveSliceMetadata defined inline and used to derive crates/diffKind', () => {
  const blockBegin = fpSrc.indexOf('function normalizePath');
  const blockEnd = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.match(block, /deriveSliceMetadata/, 'deriveSliceMetadata referenced in wiring block');
  assert.match(block, /changedCrates/, 'changedCrates derived from metadata');
  assert.match(block, /diffKind/, 'diffKind derived from metadata');
});

test('FAIL short-circuit: status precheck-failed returned before Estimate', () => {
  const precheckBegin = fpSrc.indexOf('function normalizePath');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(precheckBegin, estimatePhase);
  assert.ok(block.length > 0 && estimatePhase > precheckBegin, 'precheck precedes Estimate');
  assert.match(block, /'precheck-failed'/, "status 'precheck-failed' literal present");
  assert.match(block, /ticket\.files/, 'declared files sourced from ticket.files');
  assert.match(block, /verdict\s*===?\s*'FAIL'|verdict !== 'PASS'/, 'FAIL branch checks verdict');
});

test('FAIL short-circuit is a return (resumable status, resolveReleaseAbort と同型)', () => {
  const precheckBegin = fpSrc.indexOf('function normalizePath');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(precheckBegin, estimatePhase);
  const failIdx = block.lastIndexOf("'precheck-failed'");
  assert.ok(failIdx > 0, "status 'precheck-failed' literal present");
  const failBlock = block.slice(Math.max(0, failIdx - 300), failIdx + 300);
  assert.ok(failBlock.includes('return {'),
    'return statement carries the precheck-failed status');
  assert.match(block.slice(failIdx, failIdx + 600), /reason|undeclared|missing/,
    'FAIL return includes reason/mismatch details');
});

test('PASS injection: slice metadata referenced by Estimate prompt', () => {
  const estIdx = fpSrc.indexOf('You are the Tech Lead.');
  const estBlock = fpSrc.slice(estIdx, estIdx + 3000);
  assert.ok(estIdx > 0, 'estimate prompt exists');
  assert.match(estBlock, /precheckSliceMetadata|ticketPrecheck\.|precheckMetadata/,
    'Estimate prompt injects precheck slice metadata');
});

test('PASS injection: precheck-derived changedCrates feeds gate lanes (manual derivation replaced)', () => {
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

test('drift guard: inline evaluateTicketPrecheck matches canonical module', () => {
  const begin = fpSrc.indexOf('function normalizePath');
  const end = fpSrc.indexOf("phase('Estimate')");
  assert.ok(begin > 0 && end > begin, 'wiring block present');
  const inlineSrc = fpSrc
    .slice(begin, end)
    .replace(/^\s*\/\/.*$/gm, '');
  assert.match(inlineSrc, /function evaluateTicketPrecheckInline|const evaluateTicketPrecheck/, 'pure fn defined');

  const fnStart = inlineSrc.search(/(?:function evaluateTicketPrecheckInline|const evaluateTicketPrecheck)\s*[=(]/);
  assert.ok(fnStart >= 0, 'pure function start found');
  const helpersMatch = inlineSrc.match(/function normalizePath[\s\S]*?\n}\nfunction toNormalizedSet[\s\S]*?\n}\n/);
  assert.ok(helpersMatch, 'normalizePath + toNormalizedSet helpers inlined');
});

test('behavior: FAIL verdict produced for undeclared changed files (canonical semantics exercised via inline call shape)', () => {
  const blockBegin = fpSrc.indexOf('function normalizePath');
  const blockEnd = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(blockBegin, blockEnd).replace(/^\s*\/\/.*$/gm, '');
  const call = block.match(/evaluateTicketPrecheck(?:Inline)?\(\s*([^,)]+),\s*([^)]+)\)/);
  assert.ok(call, 'evaluateTicketPrecheck call site found');
  assert.match(call[1], /ticket\.files|declaredFiles/, 'first arg is ticket-declared files');
  assert.match(
    block,
    /precheckChangedFiles|combinedChanged|commitRangeFiles|rangeFiles/,
    'combined changed-files list (incl. commit-range fallback) built before the call'
  );
});

test('evidence: precheck verdict persisted to .omc/logs/{run-id}/ticket-precheck.json', () => {
  const persistIdx = fpSrc.indexOf('ticket-precheck.json');
  assert.ok(persistIdx > 0, 'dedicated persistence file name present');
  const around = fpSrc.slice(persistIdx - 3000, persistIdx + 500);
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.match(around, /\$\{runId\}/, 'path uses shared ${runId}');
  assert.match(around, /verdict/, 'verdict recorded in persisted JSON');
});

test('UC-3: precheck scope resolver includes commit-range fallback for clean working tree', () => {
  const blockBegin = fpSrc.indexOf('function normalizePath');
  const blockEnd = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.match(block, /HEAD~1\.\.HEAD|commit-range|merge-base/,
    'commit-range fallback (HEAD~1..HEAD / merge-base) collected by precheck scope resolver');
});

test('canonical: UC-2 fail-closed semantics unchanged (undeclared file FAILs)', () => {
  const r = evaluateTicketPrecheck(['a.rs'], ['a.rs', 'b.js']);
  assert.equal(r.verdict, 'FAIL');
  assert.deepEqual(r.undeclared, ['b.js']);
  const m = deriveSliceMetadata(['crates/x/src/lib.rs', 'docs/a.md']);
  assert.deepEqual(m.changedCrates, ['x']);
  assert.equal(m.diffKind, 'mixed');
});

test('evidence: treeHash recorded in persisted ticket-precheck.json (vacuous-PASS detection)', () => {
  const blockBegin = fpSrc.indexOf('function normalizePath');
  const blockEnd = fpSrc.indexOf("phase('Estimate')");
  const block = fpSrc.slice(blockBegin, blockEnd);
  assert.ok(blockBegin > 0 && blockEnd > blockBegin, 'wiring block present');
  assert.match(block, /git write-tree/, 'scope resolver collects git write-tree output');
  assert.match(block, /treeHash/, 'treeHash carried into persisted JSON');
  const guard = block.match(/treeHash[\s\S]{0,200}unknown/s);
  assert.ok(guard, 'missing treeHash falls back to explicit unknown (fail-closed)');
});

test('issuePremise wiring: block sits inside ticket-precheck wiring, before Estimate', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const precheckBegin = fpSrc.indexOf('function normalizePath');
  const estimatePhase = fpSrc.indexOf("phase('Estimate')");
  assert.ok(begin > 0 && end > begin, 'issue-premise wiring block present');
  assert.ok(precheckBegin < begin && end < estimatePhase,
    'issue-premise block nested inside ticket-precheck wiring block');
  assert.ok(end < estimatePhase, 'issue-premise block precedes Estimate');
});

test('issuePremise wiring: inline pure fn evaluates gh/git/PR evidence', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  assert.match(fpSrc, /const evaluateIssuePremise/, 'evaluateIssuePremise defined inline (unified block)');
  assert.match(block, /evaluateIssuePremise\(\{/, 'evaluateIssuePremise invoked with object evidence');
  assert.match(block, /gh issue view/, 'gh issue view (state/closedAt) collected');
  assert.match(block, /state/, 'issue state passed to pure fn');
  assert.match(block, /git branch -a --contains|--contains/, 'trunk membership via git branch --contains');
  assert.match(block, /gh pr list --search/, 'open-PR duplicate search collected');
});

test('issuePremise wiring: FAIL short-circuits to precheck-failed before Estimate', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
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
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  const persistIdx = block.indexOf('issue-premise-precheck.json');
  assert.ok(persistIdx > 0, 'dedicated persistence file name present in issue-premise block');
  const around = block.slice(Math.max(0, persistIdx - 2000), persistIdx + 500);
  assert.ok(around.includes('.omc/logs/'), 'log dir .omc/logs/ referenced');
  assert.match(around, /\$\{runId\}/, 'path uses shared ${runId}');
  assert.match(around, /verdict/, 'verdict recorded in persisted JSON');
});

test('issuePremise wiring: fail-closed — gh failure surfaces as FAIL via malformed evidence', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  assert.match(block, /issueState/, 'issueState field mapped from gh evidence');
  assert.match(block, /linkedBranchesContainIssue/, 'linkedBranchesContainIssue field mapped');
  assert.match(block, /openPRs/, 'openPRs field mapped');
  const bypass = block.match(/verdict\s*=\s*'PASS'/);
  assert.equal(bypass, null, 'no hardcoded PASS bypass of evaluateIssuePremise');
});

test('issuePremise wiring (#150): call site passes ticket.ticketKind and ticket.subjectPrNumber', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  const call = block.match(/evaluateIssuePremise\(\{[\s\S]*?\}\)/);
  assert.ok(call, 'evaluateIssuePremise({...}) call site found');
  assert.match(call[0], /ticketKind:\s*ticket\.ticketKind/,
    'ticketKind wired from PM ticket into evaluateIssuePremise');
  assert.match(call[0], /subjectPrNumber[^,\n]*ticket\.subjectPrNumber/,
    'subjectPrNumber wired from PM ticket into evaluateIssuePremise');
});

test('issuePremise wiring (#150): schema-null subjectPrNumber normalized to undefined at wiring boundary', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  const call = block.match(/evaluateIssuePremise\(\{[\s\S]*?\}\)/);
  assert.ok(call, 'call site found');
  assert.match(
    call[0],
    /subjectPrNumber:\s*\(ticket\.subjectPrNumber === null\)\s*\?\s*undefined\s*:\s*ticket\.subjectPrNumber/,
    'null subjectPrNumber mapped to undefined before the pure fn'
  );
});

test('PM schema (#150): optional subjectPrNumber declared, required array unchanged', () => {
  const begin = fpSrc.indexOf('const ticket = await agent(');
  const end = fpSrc.indexOf('const runMeta = buildRunMetadata', begin);
  assert.ok(begin >= 0 && end > begin, 'pm-ticket-files block present');
  const block = fpSrc.slice(begin, end);
  assert.match(
    block,
    /subjectPrNumber:\s*\{\s*type:\s*\['number',\s*'string',\s*'null'\]\s*\}/,
    'subjectPrNumber property declared with type [number, string, null]'
  );
  assert.match(
    block,
    /required:\s*\['title',\s*'priority',\s*'summary',\s*'files',\s*'ticketKind'\]/,
    'required array unchanged (subjectPrNumber not required)'
  );
  assert.doesNotMatch(block, /required:[^\]]*subjectPrNumber/,
    'subjectPrNumber must NOT appear in the required array');
});

test('PM prompt (#150): continuation branch instructs returning subject open PR number', () => {
  const begin = fpSrc.indexOf('const ticket = await agent(');
  const end = fpSrc.indexOf('const runMeta = buildRunMetadata', begin);
  const block = fpSrc.slice(begin, end);
  const branchAt = block.indexOf('${isBacklogPick');
  assert.ok(branchAt > 0, 'continuation branch found in PM prompt');
  const branch = block.slice(branchAt, branchAt + 3000);
  assert.match(branch, /subjectPrNumber/, 'continuation branch mentions subjectPrNumber');
  assert.match(branch, /open PR の場合[^。]*PR 番号[^。]*subjectPrNumber/s,
    'branch instructs: if target is an open PR, return its number as subjectPrNumber');
});

test('issuePremise evidence (#150): persisted JSON records ticketKind and subjectPrNumber', () => {
  const begin = fpSrc.indexOf('const issueNumberForPremise');
  const end = fpSrc.indexOf('log(`Issue premise precheck PASS');
  const block = fpSrc.slice(begin, end);
  const persistIdx = block.indexOf('issue-premise-precheck.json');
  assert.ok(persistIdx > 0, 'persistence file present in issue-premise block');
  const persist = block.slice(persistIdx, persistIdx + 2500);
  assert.match(persist, /ticketKind:\s*ticket\.ticketKind/,
    'ticketKind (exemption judgment input) recorded in evidence JSON');
  assert.match(persist, /subjectPrNumber:\s*[^,}]*ticket\.subjectPrNumber/,
    'subjectPrNumber (exemption judgment input) recorded in evidence JSON');
});
