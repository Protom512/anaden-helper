// Unit + integration tests for scripts/quality/duplication_gate.mjs
// Contract source: approved plan Phase 0 item 3 (Issue #214, T2).
// Run: node --test scripts/quality/duplication_gate.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  parseSimilarityOutput,
  isAllowlisted,
  evaluateDuplication,
  main,
} from './duplication_gate.mjs';

const SAMPLE_OUTPUT = [
  'Analyzing Rust code similarity...',
  '',
  '=== Function Similarity ===',
  'Checking 128 files for duplicates...',
  '',
  'Duplicates in crates\\demo\\src\\lib.rs:',
  '------------------------------------------------------------',
  '  crates\\demo\\src\\lib.rs:10-40 function alpha <-> crates\\demo\\src\\lib.rs:50-80 function beta',
  '  Similarity: 91.23%',
  '  Classes: Demo <-> Demo',
  '',
  'Duplicates in crates\\demo\\src\\other.rs:',
  '------------------------------------------------------------',
  '  crates\\demo\\src\\other.rs:1-30 function gamma <-> crates\\demo\\src\\other.rs:40-70 function delta',
  '  Similarity: 82.00%',
  '',
].join('\r\n');

const PROD_SOURCE = [
  'pub fn alpha() {}', // 1
  '', // 2
  '#[cfg(test)]', // 3
  'mod tests {', // 4 <- test region starts
  '    fn helper_a() {}', // 5
  '    fn helper_b() {}', // 6
  '}', // 7 <- test region ends
].join('\n');

// ---------------------------------------------------------------------------
// parseSimilarityOutput
// ---------------------------------------------------------------------------

test('parseSimilarityOutput: parses pairs with ranges, kinds and similarity; ignores headers and Classes lines', () => {
  const pairs = parseSimilarityOutput(SAMPLE_OUTPUT);
  assert.equal(pairs.length, 2);
  assert.equal(pairs[0].a.file, 'crates/demo/src/lib.rs');
  assert.equal(pairs[0].a.start, 10);
  assert.equal(pairs[0].a.end, 40);
  assert.equal(pairs[0].a.kind, 'function');
  assert.equal(pairs[0].a.name, 'alpha');
  assert.equal(pairs[0].b.name, 'beta');
  assert.ok(Math.abs(pairs[0].similarity - 0.9123) < 1e-9);
  assert.ok(Math.abs(pairs[1].similarity - 0.82) < 1e-9);
});

test('parseSimilarityOutput: no duplicates -> empty array (not a crash)', () => {
  const pairs = parseSimilarityOutput('Analyzing Rust code similarity...\n\n=== Function Similarity ===\nChecking 1 files for duplicates...\n');
  assert.deepEqual(pairs, []);
});

test('parseSimilarityOutput: normalizes windows path separators', () => {
  const pairs = parseSimilarityOutput(
    '  crates\\demo\\src\\lib.rs:1-9 method m1 <-> crates\\demo\\src\\lib.rs:20-29 method m2\n  Similarity: 80.50%\n',
  );
  assert.equal(pairs[0].a.file, 'crates/demo/src/lib.rs');
});

// ---------------------------------------------------------------------------
// allowlist matching
// ---------------------------------------------------------------------------

const ALLOWLIST = {
  entries: [
    {
      file: 'crates/demo/src/lib.rs',
      functions: ['alpha', 'beta'],
      reason: 'signature-only false positive',
    },
    {
      file: 'crates/demo/src/win.rs',
      functions: ['click', 'postmessage_click'],
      reason: 'dispatcher vs injector',
    },
  ],
};

test('isAllowlisted: matching file + both names -> entry; order-insensitive', () => {
  const pair = {
    a: { file: 'crates/demo/src/lib.rs', name: 'beta' },
    b: { file: 'crates/demo/src/lib.rs', name: 'alpha' },
    similarity: 0.9123,
  };
  assert.ok(isAllowlisted(pair, ALLOWLIST.entries));
});

test('isAllowlisted: same file but different functions -> null', () => {
  const pair = {
    a: { file: 'crates/demo/src/lib.rs', name: 'alpha' },
    b: { file: 'crates/demo/src/lib.rs', name: 'gamma' },
    similarity: 0.9,
  };
  assert.equal(isAllowlisted(pair, ALLOWLIST.entries), null);
});

test('isAllowlisted: same functions but different file -> null', () => {
  const pair = {
    a: { file: 'crates/demo/src/elsewhere.rs', name: 'alpha' },
    b: { file: 'crates/demo/src/elsewhere.rs', name: 'beta' },
    similarity: 0.9,
  };
  assert.equal(isAllowlisted(pair, ALLOWLIST.entries), null);
});

// ---------------------------------------------------------------------------
// evaluateDuplication
// ---------------------------------------------------------------------------

function mkPair(fileA, nameA, startA, endA, nameB, startB, endB, similarity, fileB = fileA) {
  return {
    a: { file: fileA, start: startA, end: endA, kind: 'function', name: nameA },
    b: { file: fileB, start: startB, end: endB, kind: 'function', name: nameB },
    similarity,
  };
}

test('evaluateDuplication: both-production pair at/above 0.85 violates; below is advisory', () => {
  const result = evaluateDuplication({
    pairs: [
      mkPair('crates/demo/src/prod.rs', 'f1', 1, 30, 'f2', 40, 70, 0.85),
      mkPair('crates/demo/src/prod.rs', 'g1', 1, 30, 'g2', 40, 70, 0.8499),
    ],
    sources: { 'crates/demo/src/prod.rs': 'pub fn f1() {}\n' },
    allowlist: [],
  });
  assert.equal(result.violations.length, 1);
  assert.equal(result.violations[0].a.name, 'f1');
  assert.equal(result.advisory.length, 1);
  assert.equal(result.advisory[0].a.name, 'g1');
});

test('evaluateDuplication: pair with a side inside a test region is excluded from production scope', () => {
  // helper_a (line 5) and helper_b (line 6) sit inside the cfg(test) region [4,7]
  const result = evaluateDuplication({
    pairs: [
      mkPair('crates/demo/src/lib.rs', 'helper_a', 5, 5, 'helper_b', 6, 6, 0.95),
      mkPair('crates/demo/src/lib.rs', 'alpha', 1, 1, 'prod_x', 2, 2, 0.86),
    ],
    sources: { 'crates/demo/src/lib.rs': PROD_SOURCE },
    allowlist: [],
  });
  assert.equal(result.violations.length, 1);
  assert.equal(result.violations[0].a.name, 'alpha');
  assert.equal(result.testExcluded.length, 1);
  assert.equal(result.testExcluded[0].a.name, 'helper_a');
});

test('evaluateDuplication: tests/ and benches/ pairs are out of scope regardless of similarity', () => {
  const result = evaluateDuplication({
    pairs: [
      mkPair('crates/demo/tests/integ.rs', 't1', 1, 30, 't2', 40, 70, 0.99),
      mkPair('crates/demo/benches/b.rs', 'b1', 1, 30, 'b2', 40, 70, 0.99),
    ],
    sources: {},
    allowlist: [],
  });
  assert.equal(result.violations.length, 0);
  assert.equal(result.testExcluded.length, 1);
  assert.equal(result.benchOutOfScope.length, 1);
});

test('evaluateDuplication: allowlisted >=0.85 pair is exempted but visible', () => {
  const result = evaluateDuplication({
    pairs: [mkPair('crates/demo/src/lib.rs', 'alpha', 1, 1, 'beta', 2, 2, 0.9123)],
    sources: { 'crates/demo/src/lib.rs': 'pub fn alpha() {}\n' },
    allowlist: ALLOWLIST.entries,
  });
  assert.equal(result.violations.length, 0);
  assert.equal(result.allowlisted.length, 1);
  assert.equal(result.allowlisted[0].reason, 'signature-only false positive');
});

test('evaluateDuplication: missing source for a scanned pair file throws (fail-closed contract)', () => {
  assert.throws(
    () =>
      evaluateDuplication({
        pairs: [mkPair('crates/demo/src/gone.rs', 'x', 1, 9, 'y', 20, 29, 0.9)],
        sources: {},
        allowlist: [],
      }),
    /missing source/,
  );
});

// ---------------------------------------------------------------------------
// CLI integration (main) with tmpdir fixtures — similarity-rs is NOT executed
// ---------------------------------------------------------------------------

function makeTmpRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'dup-gate-'));
}

const FIXTURE_OUTPUT = [
  'Analyzing Rust code similarity...',
  '',
  '=== Function Similarity ===',
  'Duplicates in src\\lib.rs:',
  '------------------------------------------------------------',
  '  src\\lib.rs:1-20 function dup_a <-> src\\lib.rs:30-50 function dup_b',
  '  Similarity: 90.00%',
  '  Classes: Lib <-> Lib',
  '',
].join('\n');

test('main: violation fixture exits 1 and writes report', () => {
  const root = makeTmpRoot();
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src', 'lib.rs'), 'pub fn dup_a() {}\n');
    const code = main([], {
      root,
      similarityOutput: FIXTURE_OUTPUT,
      allowlist: { entries: [] },
    });
    assert.equal(code, 1);
    const reportPath = path.join(root, 'target', 'quality', 'duplication-report.json');
    assert.ok(fs.existsSync(reportPath));
    const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
    assert.equal(report.counts.violations, 1);
    assert.equal(report.violations[0].a.name, 'dup_a');
    assert.ok(Math.abs(report.violations[0].similarity - 0.9) < 1e-9);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main: clean fixture exits 0; advisory-only does not fail', () => {
  const root = makeTmpRoot();
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src', 'lib.rs'), 'pub fn dup_a() {}\n');
    const advisoryOutput = FIXTURE_OUTPUT.replace('90.00%', '83.00%');
    const code = main([], {
      root,
      similarityOutput: advisoryOutput,
      allowlist: { entries: [] },
    });
    assert.equal(code, 0);
    const report = JSON.parse(
      fs.readFileSync(path.join(root, 'target', 'quality', 'duplication-report.json'), 'utf8'),
    );
    assert.equal(report.counts.violations, 0);
    assert.equal(report.counts.advisory, 1);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main: malformed allowlist json fails closed with exit 2', () => {
  const root = makeTmpRoot();
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src', 'lib.rs'), 'pub fn dup_a() {}\n');
    fs.writeFileSync(path.join(root, 'duplication-allowlist.json'), '{ not json');
    const code = main([], { root, similarityOutput: FIXTURE_OUTPUT });
    assert.equal(code, 2);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
