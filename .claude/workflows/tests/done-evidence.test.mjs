// TDD tests for the S2 Done-evidence pure block (Issue #68 Task 1).
// Same extraction pattern as release-precheck.test.mjs: eval the
// marker-delimited pure block shipped in feature-pipeline.js.
//
// Run: node --test .claude/workflows/tests/  (or node this file)
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [done-evidence-begin]';
const END = '// [done-evidence-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);

let api = null;
test('markers present in feature-pipeline.js', () => {
  if (begin < 0 || end <= begin) return;
  const block = src.slice(begin + BEGIN.length, end);
  api = new Function(`${block}; return { evaluateDoneEvidence, formatEvidenceForReviewers };`)();
});

const GOOD = {
  buildExitCode: 0, nextestExitCode: 0,
  passed: 120, failed: 0, skipped: 3,
  checkoutClean: true, collectedAt: '2026-08-24T00:00:00Z',
  // Issue #97 UC-3: runTimestamp = pipeline run 開始時刻 (run metadata)。
  // collectedAt (採取時刻) とは別物 — build/test の freshness 判定は collectedAt、
  //「どの run の evidence か」の traceability は runTimestamp。
  runTimestamp: '2026-08-24T00:00:00Z',
};

const run = () => {
  const block = src.slice(src.indexOf(BEGIN) + BEGIN.length, src.indexOf(END));
  return new Function(`${block}; return { evaluateDoneEvidence, formatEvidenceForReviewers };`)();
};

test('markers exist and block evaluates', () => {
  if (begin < 0) throw new Error('missing done-evidence-begin marker');
  if (end <= begin) throw new Error('missing done-evidence-end marker');
  api = run();
});

test('all-good evidence → done=true', () => {
  const r = api.evaluateDoneEvidence({ ...GOOD });
  if (r.done !== true) throw new Error(`expected done=true, got ${JSON.stringify(r)}`);
  if (r.reason !== null) throw new Error('reason should be null on success');
  if (typeof r.evidenceSummary !== 'string' || r.evidenceSummary.length === 0) {
    throw new Error('evidenceSummary must be non-empty string');
  }
});

test('missing each required field → done=false, reason=missing-evidence (runTimestamp has its own reason)', () => {
  for (const k of ['buildExitCode', 'nextestExitCode', 'passed', 'failed', 'checkoutClean', 'collectedAt']) {
    const input = { ...GOOD };
    delete input[k];
    const r = api.evaluateDoneEvidence(input);
    if (r.done !== false || r.reason !== 'missing-evidence') {
      throw new Error(`field ${k}: expected missing-evidence, got ${JSON.stringify(r)}`);
    }
  }
  // runTimestamp 欠損は専用 reason (UC-3)
  const input = { ...GOOD };
  delete input.runTimestamp;
  const r = api.evaluateDoneEvidence(input);
  if (r.done !== false || r.reason !== 'missing-run-timestamp') {
    throw new Error(`runTimestamp: expected missing-run-timestamp, got ${JSON.stringify(r)}`);
  }
});

test('null field → missing-evidence (skipped exempt)', () => {
  let r = api.evaluateDoneEvidence({ ...GOOD, buildExitCode: null });
  if (r.done !== false || r.reason !== 'missing-evidence') throw new Error('null buildExitCode');
  r = api.evaluateDoneEvidence({ ...GOOD, skipped: null });
  if (r.done !== true) throw new Error('null skipped must be tolerated');
  r = api.evaluateDoneEvidence({ ...GOOD, skipped: undefined });
  if (r.done !== true) throw new Error('absent skipped must be tolerated');
});

test('non-zero build exit → build-failed', () => {
  const r = api.evaluateDoneEvidence({ ...GOOD, buildExitCode: 101 });
  if (r.done !== false || r.reason !== 'build-failed') throw new Error(JSON.stringify(r));
});

test('non-zero nextest exit or failed>0 → tests-failed', () => {
  let r = api.evaluateDoneEvidence({ ...GOOD, nextestExitCode: 1 });
  if (r.done !== false || r.reason !== 'tests-failed') throw new Error('exit code path');
  r = api.evaluateDoneEvidence({ ...GOOD, failed: 2 });
  if (r.done !== false || r.reason !== 'tests-failed') throw new Error('failed-count path');
});

test('dirty checkout → dirty-checkout', () => {
  const r = api.evaluateDoneEvidence({ ...GOOD, checkoutClean: false });
  if (r.done !== false || r.reason !== 'dirty-checkout') throw new Error(JSON.stringify(r));
});

test('null / malformed input → fail-closed', () => {
  let r = api.evaluateDoneEvidence(null);
  if (r.done !== false || r.reason !== 'missing-evidence') throw new Error('null input');
  r = api.evaluateDoneEvidence('nope');
  if (r.done !== false || r.reason !== 'missing-evidence') throw new Error('string input');
});

test('precedence: build-failed beats tests-failed beats dirty-checkout', () => {
  let r = api.evaluateDoneEvidence({ ...GOOD, buildExitCode: 1, nextestExitCode: 1, checkoutClean: false });
  if (r.reason !== 'build-failed') throw new Error('build precedence');
  r = api.evaluateDoneEvidence({ ...GOOD, nextestExitCode: 1, checkoutClean: false });
  if (r.reason !== 'tests-failed') throw new Error('tests precedence');
});

test('negative / non-number counts → missing-evidence', () => {
  let r = api.evaluateDoneEvidence({ ...GOOD, passed: -1 });
  if (r.reason !== 'missing-evidence') throw new Error('negative passed');
  r = api.evaluateDoneEvidence({ ...GOOD, failed: 'x' });
  if (r.reason !== 'missing-evidence') throw new Error('non-number failed');
});

// ── Issue #97 UC-3: runTimestamp fail-closed ──

test('missing runTimestamp → done=false, reason=missing-run-timestamp (UC-3 fail-closed)', () => {
  const input = { ...GOOD };
  delete input.runTimestamp;
  const r = api.evaluateDoneEvidence(input);
  if (r.done !== false) throw new Error(`expected done=false, got ${JSON.stringify(r)}`);
  if (r.reason !== 'missing-run-timestamp') throw new Error(`expected missing-run-timestamp, got ${r.reason}`);
});

test('empty-string runTimestamp → missing-run-timestamp (UC-3 fail-closed)', () => {
  const r = api.evaluateDoneEvidence({ ...GOOD, runTimestamp: '' });
  if (r.done !== false || r.reason !== 'missing-run-timestamp') throw new Error(JSON.stringify(r));
});

test('runTimestamp precedence: beats build-failed but not field-missing (documented order)', () => {
  // runTimestamp 欠損は 'missing-run-timestamp' 固有 reason で報告される。
  // 他の必須フィールド欠損 (missing-evidence) があればそちらが優先される。
  const r = api.evaluateDoneEvidence({ ...GOOD, runTimestamp: '', buildExitCode: 1 });
  if (r.reason !== 'missing-run-timestamp') throw new Error(JSON.stringify(r));
  const r2 = api.evaluateDoneEvidence({ ...GOOD, runTimestamp: '', passed: null });
  if (r2.reason !== 'missing-evidence') throw new Error(JSON.stringify(r2));
});

test('formatEvidenceForReviewers surfaces runTimestamp in blob', () => {
  const blob = api.formatEvidenceForReviewers({ ...GOOD });
  if (!blob.includes('runTimestamp=2026-08-24T00:00:00Z')) throw new Error(blob);
});

test('formatEvidenceForReviewers marks absent runTimestamp as unknown / not collected (UC-3)', () => {
  const blob = api.formatEvidenceForReviewers({ ...GOOD, runTimestamp: '' });
  if (!/runTimestamp=unknown/i.test(blob)) throw new Error(blob);
  if (!/NOT DONE \(missing-run-timestamp\)/.test(blob)) throw new Error(blob);
});

test('formatEvidenceForReviewers produces capped plain-text blob', () => {
  const blob = api.formatEvidenceForReviewers({ ...GOOD, outputTail: 'x'.repeat(5000) });
  if (typeof blob !== 'string' || blob.length === 0) throw new Error('blob must be string');
  if (blob.includes('x'.repeat(2001))) throw new Error('outputTail must be capped');
  if (!blob.includes('120') || !blob.includes('build exit=0')) throw new Error('blob must include pass count and exit codes');
  if (!blob.includes('collectedAt')) throw new Error('blob must include collectedAt');
  // null evidence → explicit not-collected marker (fail-closed messaging)
  const missing = api.formatEvidenceForReviewers(null);
  if (!/not collected|missing/i.test(missing)) throw new Error('null evidence must say missing');
});
