import test from 'node:test';
import assert from 'node:assert/strict';
import { SEVERITY_RANK, normalizeFilePath, findingKey, mergeFindings } from '../review-gate-findings.js';

test('SEVERITY_RANK pins the escalation order', () => {
  assert.deepEqual(SEVERITY_RANK, { critical: 4, high: 3, medium: 2, low: 1 });
});

test('normalizeFilePath unifies separators and strips ./ prefixes', () => {
  assert.equal(normalizeFilePath('crates\\vision\\src\\lib.rs'), 'crates/vision/src/lib.rs');
  assert.equal(normalizeFilePath('./src/main.rs'), 'src/main.rs');
  assert.equal(normalizeFilePath('.\\.\\a\\b.md'), 'a/b.md');
  assert.equal(normalizeFilePath('already/normal.rs'), 'already/normal.rs');
  assert.equal(normalizeFilePath(null), '');
  assert.equal(normalizeFilePath(42), '');
});

test('findingKey composes file|line|category with fail-closed defaults', () => {
  assert.equal(findingKey({ file: 'a\\b.rs', line: 12, category: 'Panic' }), 'a/b.rs|12|panic');
  assert.equal(findingKey({ file: 'a.rs' }), 'a.rs|0|');
  assert.equal(findingKey({ file: 'a.rs', line: Number.NaN }), 'a.rs|0|');
  assert.equal(findingKey({}), '|0|');
  assert.equal(findingKey(null), '|0|');
});

test('mergeFindings returns empty for empty or missing input', () => {
  assert.deepEqual(mergeFindings([]), []);
  assert.deepEqual(mergeFindings(null), []);
  assert.deepEqual(mergeFindings(undefined), []);
});

test('mergeFindings dedupes by key, escalates severity, and unions reviewers', () => {
  const reviews = [
    { reviewer: 'r1', findings: [{ file: 'a.rs', line: 3, category: 'panic', severity: 'low', description: 'd1' }] },
    { reviewer: 'r2', findings: [{ file: 'a.rs', line: 3, category: 'panic', severity: 'critical', description: 'd2', fix: 'fix2' }] },
  ];
  const merged = mergeFindings(reviews);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].key, 'a.rs|3|panic');
  assert.equal(merged[0].severity, 'critical');
  assert.deepEqual(merged[0].reviewers, ['r1', 'r2']);
  assert.equal(merged[0].description, 'd1 / d2');
  assert.equal(merged[0].fix, 'fix2');
});

test('mergeFindings keeps distinct keys separate and never duplicates a reviewer', () => {
  const reviews = [
    { reviewer: 'r1', findings: [
      { file: 'a.rs', line: 3, category: 'panic', severity: 'low', description: 'd-a' },
      { file: 'a.rs', line: 9, category: 'panic', severity: 'low', description: 'd-b' },
    ] },
    { reviewer: 'r1', findings: [{ file: 'a.rs', line: 3, category: 'panic', severity: 'low', description: 'd-c' }] },
    { reviewer: 'r2', findings: [{ file: 'b.rs', line: 1, category: 'alloc', severity: 'medium', description: 'd-d' }] },
  ];
  const merged = mergeFindings(reviews);
  assert.equal(merged.length, 3);
  const first = merged.find((m) => m.key === 'a.rs|3|panic');
  assert.deepEqual(first.reviewers, ['r1']);
  assert.equal(first.description, 'd-a / d-c');
});

test('mergeFindings defaults missing reviewer to unknown', () => {
  const merged = mergeFindings([{ findings: [{ file: 'a.rs', line: 1, category: 'x', severity: 'low' }] }]);
  assert.deepEqual(merged[0].reviewers, ['unknown']);
});
