import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

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
  assert.match(src, /ACTIVE_GATE_DIMENSIONS\.map\(\(d\) => `--- teammate gate-\$\{d\.key\} へのプロンプト ---\\n\$\{d\.prompt\}/);
  assert.match(src, /parallel\(ACTIVE_GATE_DIMENSIONS\.map\(\(d\) => \(\) => runGateDimension\(d\)\)\)/);
});

test('injected instruction makes evidence citation mandatory for Done/完了 claims', () => {
  const fiAt = src.indexOf('const FEEDBACK_INSTRUCTION');
  const fiEnd = src.indexOf('GATE_DIMENSIONS', fiAt);
  const fi = src.slice(fiAt, fiEnd);
  assert.match(fi, /Done/i, 'instruction mentions Done');
  assert.match(fi, /evidence を引用|evidence.*必須/, 'evidence citation is mandatory');
  assert.match(fi, /NO-GO 扱い/, 'evidence-contradicting completion claims fail closed (treated as NO-GO)');
  assert.ok(
    fi.includes('${GATE_EVIDENCE}\n【Done/完了 判定は evidence 引用必須'),
    'blob interpolated directly above the mandatory-citation instruction within FEEDBACK_INSTRUCTION'
  );
});
