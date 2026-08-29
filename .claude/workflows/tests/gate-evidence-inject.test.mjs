// TDD tests for Issue #68 (S2) Task 4: inject the Done-evidence blob into all
// 6 gate reviewer prompts (team path + fallback parallel() path).
// Source-scan pattern — the workflow script is not importable.
//
// Run: node .claude/workflows/tests/gate-evidence-inject.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

test('markers present in feature-pipeline.js', () => {
  assert.ok(src.includes('// [gate-evidence-inject-begin]'));
  assert.ok(src.includes('// [gate-evidence-inject-end]'));
});

test('GATE_EVIDENCE blob is declared before FEEDBACK_INSTRUCTION (no TDZ at runtime)', () => {
  const evAt = src.indexOf('const GATE_EVIDENCE = formatEvidenceForReviewers(');
  const fiAt = src.indexOf('const FEEDBACK_INSTRUCTION');
  assert.ok(evAt >= 0, 'GATE_EVIDENCE built via formatEvidenceForReviewers (Task 3 blob)');
  assert.ok(fiAt >= 0, 'FEEDBACK_INSTRUCTION exists');
  assert.ok(evAt < fiAt, 'GATE_EVIDENCE precedes FEEDBACK_INSTRUCTION');
});

test('blob is injected into FEEDBACK_INSTRUCTION (shared by all 6 dimension prompts)', () => {
  const fiAt = src.indexOf('const FEEDBACK_INSTRUCTION');
  const fiEnd = src.indexOf('GATE_DIMENSIONS', fiAt);
  const fi = src.slice(fiAt, fiEnd);
  assert.ok(fi.includes('${GATE_EVIDENCE}'), 'GATE_EVIDENCE interpolated inside FEEDBACK_INSTRUCTION');
  const uses = (src.match(/\$\{FEEDBACK_INSTRUCTION\}/g) || []).length;
  assert.equal(uses, 6, `all 6 GATE_DIMENSIONS prompts embed FEEDBACK_INSTRUCTION (found ${uses})`);
});

test('both gate paths consume d.prompt (so the injection reaches team AND fallback routes)', () => {
  // P-008 (Issue #95): lane set is ACTIVE_GATE_DIMENSIONS (docs-only short-circuit
  // filters a subset); both paths must consume the ACTIVE set so injected evidence
  // still reaches every lane that actually runs.
  // team path: spawner embeds d.prompt per teammate
  assert.match(src, /ACTIVE_GATE_DIMENSIONS\.map\(\(d\) => `--- teammate gate-\$\{d\.key\} へのプロンプト ---\\n\$\{d\.prompt\}/);
  // fallback path: parallel() over dimension prompts
  assert.match(src, /parallel\(ACTIVE_GATE_DIMENSIONS\.map\(\(d\) => \(\) => runGateDimension\(d\)\)\)/);
});

test('injected instruction makes evidence citation mandatory for Done/完了 claims', () => {
  const begin = src.indexOf('// [gate-evidence-inject-begin]');
  const end = src.indexOf('// [gate-evidence-inject-end]');
  assert.ok(begin >= 0 && end > begin);
  const block = src.slice(begin, end);
  assert.match(block, /Done/i, 'instruction mentions Done');
  assert.match(block, /evidence を引用|evidence.*必須/, 'evidence citation is mandatory');
  assert.match(block, /fail-closed/, 'fail-closed semantics stated');
  // the blob interpolation immediately precedes the marker block inside FEEDBACK_INSTRUCTION
  const fiAt = src.indexOf('const FEEDBACK_INSTRUCTION');
  const fiEnd = src.indexOf('GATE_DIMENSIONS', fiAt);
  assert.ok(
    src.slice(fiAt, fiEnd).includes('${GATE_EVIDENCE}\n// [gate-evidence-inject-begin]'),
    'blob interpolated directly above the mandatory-citation block within FEEDBACK_INSTRUCTION'
  );
});
