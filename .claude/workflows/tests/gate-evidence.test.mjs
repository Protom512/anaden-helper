// TDD tests for the S2 evidence-collector gate wiring (Issue #68 Task 3).
// Same extraction pattern as release-precheck.test.mjs: the workflow script is
// not an importable module, so we eval the marker-delimited pure block that
// ships in feature-pipeline.js. Done-evaluation semantics themselves are
// covered by done-evidence.test.mjs (Task 1); this file covers Task 3:
// normalizeGateEvidence (agent-output transport) + source wiring invariants.
//
// Run: node --test .claude/workflows/tests/gate-evidence.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [gate-evidence-begin]';
const END = '// [gate-evidence-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);
assert.ok(begin >= 0, 'feature-pipeline.js contains gate-evidence-begin marker');
assert.ok(end > begin, 'feature-pipeline.js contains gate-evidence-end marker after begin');

const block = src.slice(begin + BEGIN.length, end);
const api = new Function(`${block}; return { GATE_EVIDENCE_STATUS_FAILED, GATE_EVIDENCE_SCHEMA, normalizeGateEvidence };`)();

// ── normalizeGateEvidence: structured-first, JSON-string tolerant ──

test('normalize: structured object passes through with defaults', () => {
  const e = api.normalizeGateEvidence({
    buildExitCode: 0, nextestExitCode: 0, passed: 890, failed: 0,
    checkoutClean: true, collectedAt: '2026-08-24T00:00:00Z',
    runTimestamp: '2026-08-24T00:00:00+09:00',
  });
  assert.equal(e.buildExitCode, 0);
  assert.equal(e.passed, 890);
  assert.equal(e.toolMissing, false);
  assert.equal(e.runTimestamp, '2026-08-24T00:00:00+09:00');
});

// ── Issue #97 UC-3: runTimestamp in schema + normalize ──

test('schema: runTimestamp is a required string field (UC-3)', () => {
  assert.equal(api.GATE_EVIDENCE_SCHEMA.properties.runTimestamp.type, 'string');
  assert.ok(api.GATE_EVIDENCE_SCHEMA.required.includes('runTimestamp'), 'runTimestamp in required');
});

test('normalize: missing runTimestamp → empty string (fail-closed sentinel, UC-3)', () => {
  const e = api.normalizeGateEvidence({ buildExitCode: 0, nextestExitCode: 0, checkoutClean: true, collectedAt: 't' });
  assert.equal(e.runTimestamp, '');
});

test('normalize: non-string runTimestamp coerced to empty string', () => {
  const e = api.normalizeGateEvidence({ buildExitCode: 0, nextestExitCode: 0, checkoutClean: true, runTimestamp: 123 });
  assert.equal(e.runTimestamp, '');
});

test('normalize: JSON string body (fenced or bare) is parsed', () => {
  const e = api.normalizeGateEvidence('```json\n{"buildExitCode":0,"nextestExitCode":0,"passed":5,"failed":0,"checkoutClean":true,"collectedAt":"t"}\n```');
  assert.equal(e.passed, 5);
  const e2 = api.normalizeGateEvidence('{"buildExitCode":0,"nextestExitCode":0,"passed":5,"failed":0,"checkoutClean":true,"collectedAt":"t"}');
  assert.equal(e2.passed, 5);
});

test('normalize: null on missing/garbage input (fail-closed)', () => {
  assert.equal(api.normalizeGateEvidence(null), null);
  assert.equal(api.normalizeGateEvidence('no json here'), null);
  assert.equal(api.normalizeGateEvidence(42), null);
});

test('normalize: numbers coerced, non-numeric exit codes → null', () => {
  const e = api.normalizeGateEvidence({ buildExitCode: '0', nextestExitCode: 1, checkoutClean: true });
  assert.equal(e.buildExitCode, 0);
  assert.equal(e.nextestExitCode, 1);
  assert.equal(api.normalizeGateEvidence({ buildExitCode: 'x' }), null);
});

test('normalize: toolMissing preserved for disambiguation', () => {
  const e = api.normalizeGateEvidence({ buildExitCode: 0, nextestExitCode: 127, checkoutClean: true, toolMissing: true, tool: 'cargo-nextest' });
  assert.equal(e.toolMissing, true);
  assert.equal(e.tool, 'cargo-nextest');
});

// ── tool-missing vs tests-failed disambiguation flows through evaluateDoneEvidence ──

test('evaluate (via done-evidence block): tool-missing reason is distinct from tests-failed', () => {
  const db = src.slice(src.indexOf('// [done-evidence-begin]') + '// [done-evidence-begin]'.length, src.indexOf('// [done-evidence-end]'));
  const done = new Function(`${db}; return { evaluateDoneEvidence, formatEvidenceForReviewers };`)();
  const GOOD = { buildExitCode: 0, nextestExitCode: 0, passed: 10, failed: 0, checkoutClean: true, collectedAt: 't', runTimestamp: '2026-08-28T00:00:00Z' };
  const tests = done.evaluateDoneEvidence({ ...GOOD, nextestExitCode: 1, failed: 3 });
  assert.equal(tests.reason, 'tests-failed');
  const tool = done.evaluateDoneEvidence({ ...GOOD, nextestExitCode: 127, toolMissing: true, tool: 'cargo-nextest' });
  assert.equal(tool.done, false);
  assert.equal(tool.reason, 'tool-missing');
});

// ── wiring structure assertions (source-level invariants) ──

test('wiring: evidence agent spawned with gate:evidence label, sonnet, Commit Gate phase', () => {
  assert.match(src, /label:\s*'gate:evidence'/, 'gate:evidence label');
  const idx = src.indexOf("label: 'gate:evidence'");
  const ctx = src.slice(idx, idx + 200);
  assert.match(ctx, /model:\s*'sonnet'/, 'sonnet model on evidence agent');
  assert.match(ctx, /phase:\s*'Commit Gate'/, 'Commit Gate phase on evidence agent');
});

test('wiring: evidence collection runs after R7 snapshot and before reviewer spawn', () => {
  const snap = src.indexOf('snapshot:r7-commit');
  const ev = src.indexOf("label: 'gate:evidence'");
  const reviewers = src.indexOf('runGateDimension');
  assert.ok(snap >= 0 && ev > snap, 'evidence agent after R7 snapshot commit');
  assert.ok(reviewers > ev, 'evidence agent before reviewer spawn');
});

test('wiring: done=false short-circuits with resumable evidence-failed status before reviewers', () => {
  const evShort = src.indexOf('evidenceShortCircuit');
  assert.ok(evShort >= 0, 'short-circuit branch exists');
  assert.ok(evShort > src.indexOf("label: 'gate:evidence'"), 'short-circuit after evidence collection');
  assert.ok(evShort < src.indexOf('runCommitGateViaTeam'), 'short-circuit before gate team spawn');
  assert.match(src, /status:\s*GATE_EVIDENCE_STATUS_FAILED/, 'returns structured evidence-failed status');
  assert.match(src, /snapshotBranch/, 'snapshot branch surfaced for resumability');
});

test('wiring: evidence blob injected into reviewer prompts (both routes)', () => {
  assert.match(src, /const GATE_EVIDENCE = formatEvidenceForReviewers\(/, 'formatted evidence feeds reviewer context');
  assert.match(src, /\$\{GATE_EVIDENCE\}/, 'GATE_EVIDENCE interpolated into FEEDBACK_INSTRUCTION');
});

test('wiring: agent prompt demands fresh worktree checkout and both cargo commands', () => {
  const evPromptIdx = src.indexOf('EVIDENCE COLLECTOR');
  assert.ok(evPromptIdx >= 0, 'evidence agent prompt present');
  const promptSlice = src.slice(evPromptIdx, evPromptIdx + 4000);
  assert.match(promptSlice, /worktree/, 'fresh worktree required');
  assert.match(promptSlice, /cargo build --workspace --all-targets/, 'build command');
  assert.match(promptSlice, /cargo nextest run --workspace --all-targets/, 'nextest command');
  assert.match(promptSlice, /toolMissing/, 'tool-missing disambiguation field demanded');
});
