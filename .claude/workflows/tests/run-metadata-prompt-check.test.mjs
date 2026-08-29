// TDD tests for Issue #97 Task 2: Commit Gate / Release Review / evidence collector
// の各 agent プロンプトへ runTimestamp 照合指示 (run-metadata.json の runTimestamp を
// 読み、evidence の recordedAt と照合。メタデータ欠損ディレクトリは unknown として
// fail-closed。遡及補填はしない — Issue #97 非スコープ) を追記したかの drift-guard。
//
// Source-scan pattern (workflow script is not importable):
//   Run: node --test .claude/workflows/tests/run-metadata-prompt-check.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

// marker block: prompt 共通指示 (Issue #97 Task 2)
const RUN_TS_CHECK_BEGIN = 'run-metadata-prompt-check-begin';
const RUN_TS_CHECK_END = 'run-metadata-prompt-check-end';

function markerBlock() {
  const begin = src.indexOf(RUN_TS_CHECK_BEGIN);
  const end = src.indexOf(RUN_TS_CHECK_END);
  assert.ok(begin > -1, 'prompt-check marker begin present');
  assert.ok(end > begin, 'prompt-check marker end present');
  return src.slice(begin, end);
}

test('drift guard: run-metadata-prompt-check marker block present', () => {
  markerBlock();
});

test('marker block states the runTimestamp cross-check semantics (read run-metadata.json, compare with recordedAt, unknown fail-closed, no backfill)', () => {
  const block = markerBlock();
  assert.match(block, /run-metadata\.json/, 'mentions run-metadata.json');
  assert.match(block, /runTimestamp/, 'mentions runTimestamp');
  assert.match(block, /recordedAt/, 'mentions recordedAt');
  assert.match(block, /照合/, 'mentions cross-check (照合)');
  assert.match(block, /unknown/, 'mentions unknown classification');
  assert.match(block, /fail-closed/, 'mentions fail-closed');
  assert.match(block, /遡及補填はしない|遡及補填しない/, 'explicitly no retroactive backfill (Issue #97 non-scope)');
});

test('Commit Gate reviewer prompts embed the runTimestamp check instruction', () => {
  const block = markerBlock();
  // FEEDBACK_INSTRUCTION (shared by all gate dimension prompts) must interpolate the
  // marker block constant so every gate lane receives the instruction.
  const fiAt = src.indexOf('const FEEDBACK_INSTRUCTION');
  assert.ok(fiAt > -1, 'FEEDBACK_INSTRUCTION exists');
  const fiEnd = src.indexOf('GATE_DIMENSIONS', fiAt);
  const fi = src.slice(fiAt, fiEnd);
  assert.ok(fi.includes('${RUN_TS_CHECK}'), 'FEEDBACK_INSTRUCTION interpolates RUN_TS_CHECK');
  assert.ok(src.includes('const RUN_TS_CHECK'), 'RUN_TS_CHECK constant declared');
  // declared before FEEDBACK_INSTRUCTION (no TDZ)
  assert.ok(src.indexOf('const RUN_TS_CHECK') < fiAt, 'RUN_TS_CHECK precedes FEEDBACK_INSTRUCTION');
  // the constant's content is derived from the marker block region
  const declAt = src.indexOf('const RUN_TS_CHECK');
  const declEnd = src.indexOf(';', declAt);
  assert.ok(declEnd > declAt);
});

test('evidence collector prompt instructs reading runTimestamp from run-metadata.json and reporting it', () => {
  const ecAt = src.indexOf('EVIDENCE COLLECTOR');
  assert.ok(ecAt > -1, 'EVIDENCE COLLECTOR prompt exists');
  const ecEnd = src.indexOf("label: 'gate:evidence'", ecAt);
  assert.ok(ecEnd > ecAt);
  const ec = src.slice(ecAt, ecEnd);
  assert.match(ec, /run-metadata\.json/, 'collector reads run-metadata.json');
  assert.match(ec, /runTimestamp/, 'collector reports runTimestamp (not self-generated)');
  assert.match(ec, /recordedAt/, 'collector sets recordedAt on evidence');
  assert.match(ec, /unknown/, 'collector treats missing metadata as unknown (fail-closed)');
});

test('Release Review reviewer prompts (team path + fallback path) embed the runTimestamp check instruction', () => {
  // both reviewer prompt constructions must include RUN_TS_CHECK
  const uses = (src.match(/\$\{RUN_TS_CHECK\}/g) || []).length;
  // 1 in FEEDBACK_INSTRUCTION + 1 team reviewerPrompt + 1 fallback parallel() prompt = 3
  assert.ok(uses >= 3, `RUN_TS_CHECK interpolated in >=3 prompts (found ${uses})`);
  // team path prompt
  const teamAt = src.indexOf('RELEASE REVIEWER teammate');
  assert.ok(teamAt > -1, 'team reviewer prompt exists');
  const teamPrompt = src.slice(teamAt, src.indexOf('PR 情報', teamAt));
  assert.ok(teamPrompt.includes('${RUN_TS_CHECK}'), 'team reviewer prompt embeds RUN_TS_CHECK');
  // fallback path prompt
  const fbAt = src.indexOf('RELEASE REVIEWER ${i + 1}/3');
  assert.ok(fbAt > -1, 'fallback reviewer prompt exists');
  const fbPrompt = src.slice(fbAt, src.indexOf('PR 情報', fbAt));
  assert.ok(fbPrompt.includes('${RUN_TS_CHECK}'), 'fallback reviewer prompt embeds RUN_TS_CHECK');
});
