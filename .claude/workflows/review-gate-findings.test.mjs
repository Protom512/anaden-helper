// Tests for review-gate findings normalization/dedup (Issue #79 Task 1 — TDD, tests BEFORE impl).
// Run: node --test .claude/workflows/review-gate-findings.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { normalizeFilePath, findingKey, mergeFindings, SEVERITY_RANK } from './review-gate-findings.js';

const here = dirname(fileURLToPath(import.meta.url));

// ── normalizeFilePath: separator + relative-path normalization ──

test('normalizeFilePath: backslashes unified to slashes', () => {
  assert.equal(normalizeFilePath('crates\\foo\\src\\lib.rs'), 'crates/foo/src/lib.rs');
});

test('normalizeFilePath: leading ./ stripped', () => {
  assert.equal(normalizeFilePath('./crates/foo/src/lib.rs'), 'crates/foo/src/lib.rs');
});

test('normalizeFilePath: repeated ./ and .\\ collapsed', () => {
  assert.equal(normalizeFilePath('.\\.\\crates\\foo.rs'), 'crates/foo.rs');
});

test('normalizeFilePath: null/undefined/empty → empty string (fail-open)', () => {
  assert.equal(normalizeFilePath(null), '');
  assert.equal(normalizeFilePath(undefined), '');
  assert.equal(normalizeFilePath(''), '');
});

// ── findingKey: file + line + category composite key ──

test('findingKey: file/line/category composite', () => {
  assert.equal(findingKey({ file: 'a\\b.rs', line: 10, category: 'unwrap' }), 'a/b.rs|10|unwrap');
});

test('findingKey: line undefined falls back to 0 (file+category scope)', () => {
  assert.equal(findingKey({ file: 'a/b.rs', category: 'unwrap' }), 'a/b.rs|0|unwrap');
});

test('findingKey: same file different path style + missing line still collide', () => {
  assert.equal(
    findingKey({ file: './a/b.rs', line: 10, category: 'unwrap' }),
    findingKey({ file: 'a\\b.rs', line: 10, category: 'unwrap' })
  );
  assert.equal(
    findingKey({ file: 'a/b.rs', category: 'unwrap' }),
    findingKey({ file: 'a\\b.rs', category: 'unwrap' })
  );
});

test('findingKey: different category → different key (dedup is per issue-kind)', () => {
  assert.notEqual(
    findingKey({ file: 'a.rs', line: 1, category: 'unwrap' }),
    findingKey({ file: 'a.rs', line: 1, category: 'missing-doc' })
  );
});

// ── SEVERITY_RANK: critical > high > medium > low ──

test('SEVERITY_RANK: ordering critical>high>medium>low', () => {
  assert.ok(SEVERITY_RANK.critical > SEVERITY_RANK.high);
  assert.ok(SEVERITY_RANK.high > SEVERITY_RANK.medium);
  assert.ok(SEVERITY_RANK.medium > SEVERITY_RANK.low);
});

// ── mergeFindings: dedup across reviewers with severity max arbitration ──

test('mergeFindings: dedups identical findings across reviewers', () => {
  const reviews = [
    { reviewer: 'review:architecture', findings: [
      { severity: 'medium', description: 'unwrap in lib', file: 'crates/a/src/lib.rs', line: 42, category: 'unwrap' },
    ]},
    { reviewer: 'review:maintainability', findings: [
      { severity: 'high', description: 'unwrap() used in library code', file: 'crates\\a\\src\\lib.rs', line: 42, category: 'unwrap' },
    ]},
  ];
  const merged = mergeFindings(reviews);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].severity, 'high'); // max severity wins
  assert.deepEqual(merged[0].reviewers.sort(), ['review:architecture', 'review:maintainability']);
  assert.ok(merged[0].description.includes('unwrap in lib'));
  assert.ok(merged[0].description.includes('unwrap() used in library code'));
});

test('mergeFindings: keeps distinct findings separate', () => {
  const reviews = [
    { reviewer: 'r1', findings: [
      { severity: 'low', description: 'A', file: 'a.rs', line: 1, category: 'unwrap' },
      { severity: 'low', description: 'B', file: 'a.rs', line: 2, category: 'unwrap' },
    ]},
  ];
  assert.equal(mergeFindings(reviews).length, 2);
});

test('mergeFindings: missing line (maintainability schema) dedups against explicit same-line later', () => {
  const reviews = [
    { reviewer: 'r1', findings: [{ severity: 'low', description: 'no line', file: 'a.rs', category: 'unwrap' }] },
    { reviewer: 'r2', findings: [{ severity: 'high', description: 'with line', file: 'a.rs', category: 'unwrap' }] },
  ];
  const merged = mergeFindings(reviews);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].severity, 'high');
  assert.equal(merged[0].line, 0);
});

test('mergeFindings: severity never downgrades when merged (max arbitration)', () => {
  const reviews = [
    { reviewer: 'r1', findings: [{ severity: 'critical', description: 'X', file: 'a.rs', line: 1, category: 'c' }] },
    { reviewer: 'r2', findings: [{ severity: 'low', description: 'Y', file: 'a.rs', line: 1, category: 'c' }] },
    { reviewer: 'r3', findings: [{ severity: 'medium', description: 'Z', file: 'a.rs', line: 1, category: 'c' }] },
  ];
  assert.equal(mergeFindings(reviews)[0].severity, 'critical');
});

test('mergeFindings: empty/missing findings handled fail-open', () => {
  assert.deepEqual(mergeFindings([]), []);
  assert.deepEqual(mergeFindings([{ reviewer: 'r1' }]), []);
  assert.deepEqual(mergeFindings(null), []);
});

test('mergeFindings: preserves fix strings (first non-empty wins)', () => {
  const reviews = [
    { reviewer: 'r1', findings: [{ severity: 'low', description: 'A', file: 'a.rs', line: 1, category: 'c', fix: '' }] },
    { reviewer: 'r2', findings: [{ severity: 'high', description: 'B', file: 'a.rs', line: 1, category: 'c', fix: 'use ok_or' }] },
  ];
  const merged = mergeFindings(reviews);
  assert.equal(merged[0].fix, 'use ok_or');
});

// ── drift guard: review-gate.js inlines the canonical helpers (R8 pattern) ──

const gateSrc = readFileSync(join(here, 'review-gate.js'), 'utf8');

test('review-gate.js: inlines canonical findings helpers (ESM import rejected by runtime)', () => {
  assert.ok(gateSrc.includes('function findingKey('));
  assert.ok(gateSrc.includes('function mergeFindings('));
  assert.ok(gateSrc.includes('function normalizeFilePath('));
});

test('review-gate.js: applies mergeFindings to reviews before Judge phase', () => {
  assert.ok(gateSrc.includes('mergeFindings(validReviews)'));
});

test('review-gate.js: maintainability findings schema includes line and category', () => {
  // L226-area schema: line + category added so keys are deterministic
  const maint = gateSrc.split("review:maintainability'")[1] ?? '';
  assert.ok(maint.includes('line:'));
  assert.ok(maint.includes('category:'));
});

test('review-gate.js: all reviewer findings schemas include category field', () => {
  const count = (gateSrc.match(/category: \{ type: 'string' \}/g) || []).length;
  assert.ok(count >= 3, `expected >=3 category fields in schemas, got ${count}`);
});

// ── drift guard (Task 4): inline copies in review-gate.js behave identically
//    to the canonical module review-gate-findings.js — same pattern as
//    review-gate-diff-inject.test.mjs. The Workflow runtime rejects ESM
//    imports, so review-gate.js carries an inline copy; this test catches
//    drift when only one side is edited. ──

// Extract a top-level `function NAME(...) {...}` (or `const NAME = {...}`) body
// from review-gate.js by brace matching.
function extractInline(name) {
  const fnAt = gateSrc.indexOf(`function ${name}(`);
  assert.ok(fnAt >= 0, `review-gate.js must inline function ${name}(`);
  const open = gateSrc.indexOf('{', fnAt);
  let depth = 0;
  for (let i = open; i < gateSrc.length; i++) {
    if (gateSrc[i] === '{') depth++;
    else if (gateSrc[i] === '}') {
      depth--;
      if (depth === 0) return gateSrc.slice(fnAt, i + 1);
    }
  }
  assert.fail(`unbalanced braces while extracting ${name}`);
}

const inlineRank = /const SEVERITY_RANK = (\{[^}]+\});/.exec(gateSrc)?.[1];
assert.ok(inlineRank, 'review-gate.js must inline SEVERITY_RANK');
// Evaluate the inline copies in isolation (no Workflow runtime needed).
const inline = new Function(`
  const SEVERITY_RANK = ${inlineRank};
  ${extractInline('normalizeFilePath')}
  ${extractInline('findingKey')}
  ${extractInline('severityRankOf')}
  ${extractInline('mergeFindings')}
  return { normalizeFilePath, findingKey, mergeFindings, SEVERITY_RANK };
`)();

test('drift guard: SEVERITY_RANK inline copy deep-equals canonical module', () => {
  assert.deepEqual(inline.SEVERITY_RANK, SEVERITY_RANK);
});

test('drift guard: inline normalizeFilePath matches canonical on corpus', () => {
  const corpus = [
    'crates\\a\\src\\lib.rs', './crates/a.rs', '.\\.\\x.rs', 'a/b.rs',
    '', null, undefined, 42, 'C:\\abs\\path.rs', '///dup//./x.rs',
  ];
  for (const c of corpus) {
    assert.equal(inline.normalizeFilePath(c), normalizeFilePath(c), `normalizeFilePath(${JSON.stringify(c)})`);
  }
});

test('drift guard: inline findingKey matches canonical on corpus', () => {
  const corpus = [
    { file: 'a\\b.rs', line: 10, category: 'Unwrap' },
    { file: './a/b.rs', category: 'unwrap' },
    { file: 'a.rs', line: 0, category: 'x' },
    { file: null, line: NaN, category: null },
    null,
    {},
    { file: 'a.rs', line: 3.5, category: '  spaced  ' },
  ];
  for (const c of corpus) {
    assert.equal(inline.findingKey(c), findingKey(c), `findingKey(${JSON.stringify(c)})`);
  }
});

test('drift guard: inline mergeFindings matches canonical on multi-reviewer corpus', () => {
  const corpus = [
    // cross-reviewer dup with severity escalation (max arbitration)
    [
      { reviewer: 'r1', findings: [{ severity: 'low', description: 'A', file: 'a\\b.rs', line: 1, category: 'u', fix: '' }] },
      { reviewer: 'r2', findings: [{ severity: 'critical', description: 'A2', file: './a/b.rs', line: 1, category: 'u', fix: 'use ok_or' }] },
    ],
    // line-less + line-0 collision (maintainability schema fallback)
    [
      { reviewer: 'r1', findings: [{ severity: 'low', description: 'x', file: 'a.rs', category: 'u' }] },
      { reviewer: 'r2', findings: [{ severity: 'high', description: 'y', file: 'a.rs', line: 0, category: 'u' }] },
    ],
    // distinct findings stay separate
    [
      { reviewer: 'r1', findings: [
        { severity: 'low', description: 'p', file: 'a.rs', line: 1, category: 'x' },
        { severity: 'low', description: 'q', file: 'a.rs', line: 1, category: 'y' },
      ]},
    ],
    // fail-open shapes
    [], null, [{ reviewer: 'r1' }], [{ reviewer: 'r1', findings: null }],
  ];
  for (const reviews of corpus) {
    assert.deepEqual(inline.mergeFindings(reviews), mergeFindings(reviews), `mergeFindings(${JSON.stringify(reviews)})`);
  }
});

test('drift guard: canonical module keeps no drift-only edits (source hash of canonical present inline)', () => {
  // Behavioral guards above are primary; this catches silent behavior-equal
  // rewrites that still signal the copies diverged in intent.
  const canonical = readFileSync(join(here, 'review-gate-findings.js'), 'utf8');
  for (const token of ['critical: 4', "f.category ?? ''", "review.reviewer ?? 'unknown'", 'Number.isFinite(f.line)']) {
    assert.ok(canonical.includes(token), `canonical includes ${token}`);
    assert.ok(gateSrc.includes(token), `inline copy includes ${token}`);
  }
});
