// Unit + integration tests for scripts/quality/assert_tests_untouched.mjs
// Contract source: approved plan Phase 0 item 4 (Issue #214, T3) —
// "テスト不改変" structural guarantee: no git-diff hunk may touch a test
// module region, a #[cfg(test)] item, or a tests/ directory file.
// Run: node --test scripts/quality/assert_tests_untouched.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { main } from './assert_tests_untouched.mjs';

const OLD_SRC = [
  'pub fn prod_a() {}', // 1
  'pub fn prod_b() {}', // 2
  '', // 3
  '#[cfg(test)]', // 4
  'mod tests {', // 5
  '    #[test]', // 6
  '    fn t1() { assert!(true); }', // 7
  '}', // 8
].join('\n');

// A refactor that only changes production lines (lines 1-2) in the new version.
const NEW_SRC_PROD_ONLY = OLD_SRC.replace('pub fn prod_a() {}', 'pub fn prod_a() -> u32 { 1 }');

// New version where a production fn was deleted entirely (test region intact).
const NEW_SRC_DELETED_PROD = ['pub fn prod_b() {}', '', '#[cfg(test)]', 'mod tests {', '    #[test]', '    fn t1() { assert!(true); }', '}'].join('\n');

// ---------------------------------------------------------------------------
// Core scenarios
// ---------------------------------------------------------------------------

test('added line inside a new-side test region is a violation (exit 1)', () => {
  // Modify line 7 (inside region [4..8] incl. attr) of crates/x/src/lib.rs.
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -7,1 +7,1 @@',
    '-    fn t1() { assert!(true); }',
    '+    fn t1() { assert!(false); }',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': OLD_SRC.replace('assert!(true)', 'assert!(false)') },
  });
  assert.equal(code, 1);
});

test('deleted line that was inside an old-side test region is a violation', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -6,2 +5,1 @@',
    '-    #[test]',
    '-    fn t1() { assert!(true); }',
    ' }',
  ].join('\n');
  const newSrc = ['#cfg', ''].join('\n'); // placeholder, new side won't hit region
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': OLD_SRC.replace('    #[test]\n    fn t1() { assert!(true); }\n', '') },
  });
  assert.equal(code, 1);
});

test('production-only modifications on both sides pass (exit 0)', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -1,1 +1,1 @@',
    '-pub fn prod_a() {}',
    '+pub fn prod_a() -> u32 { 1 }',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': NEW_SRC_PROD_ONLY },
  });
  assert.equal(code, 0);
});

test('deleting production lines (test region untouched) passes', () => {
  // prod_a removed: old lines 1 (production) deleted, new side shifts the rest.
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -1,2 +1,1 @@',
    '-pub fn prod_a() {}',
    ' pub fn prod_b() {}',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': NEW_SRC_DELETED_PROD },
  });
  assert.equal(code, 0);
});

test('editing the #[cfg(test)] attribute line itself is a violation', () => {
  const newSrc = OLD_SRC.replace('#[cfg(test)]', '#[cfg(all(test, windows))]');
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -4,1 +4,1 @@',
    '-#[cfg(test)]',
    '+#[cfg(all(test, windows))]',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': newSrc },
  });
  assert.equal(code, 1);
});

test('adding a brand-new #[cfg(test)] mod in a src file is a violation', () => {
  const newSrc = `${OLD_SRC}\n#[cfg(test)]\nmod more_tests {\n    #[test]\n    fn t2() {}\n}\n`;
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -8,0 +9,5 @@',
    '+#[cfg(test)]',
    '+mod more_tests {',
    '+    #[test]',
    '+    fn t2() {}',
    '+}',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': newSrc },
  });
  assert.equal(code, 1);
});

test('any change to a tests/ directory file is a violation', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    'new file mode 100644',
    '--- /dev/null',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -0,0 +1,3 @@',
    '+#[test]',
    '+fn new_test() {}',
    '+',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: {},
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nfn new_test() {}\n' },
  });
  assert.equal(code, 1);
});

test('whitespace-only rewrap of a tests/ directory file passes (Issue #272)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,4 +2,1 @@',
    '-assert_eq!(',
    '-    j.point(),',
    '-    (1, 2)',
    '-);',
    '+assert_eq!(j.point(), (1, 2));',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(\n    j.point(),\n    (1, 2)\n);\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(j.point(), (1, 2));\n' },
  });
  assert.equal(code, 0);
});

test('rustfmt array collapse that drops a trailing comma is NOT whitespace-only (Issue #272 boundary)', () => {
  // The real drift shape: multi-line array with trailing comma collapses to one
  // line and rustfmt removes the comma, so tokens change and the gate still
  // flags it. Whether to treat trailing-comma normalization as insignificant is
  // an open operator decision recorded in Issue #272.
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,5 +2,1 @@',
    '-for name in [',
    '-    "A",',
    '-    "B",',
    '-] {',
    '+for name in ["A", "B"] {',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nfor name in [\n    "A",\n    "B",\n] {}\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nfor name in ["A", "B"] {}\n' },
  });
  assert.equal(code, 1);
});

test('indent-only change inside a #[cfg(test)] region passes (Issue #272)', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -7,1 +7,1 @@',
    '-    fn t1() { assert!(true); }',
    '+      fn t1() { assert!(true); }',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': OLD_SRC.replace('    fn t1()', '      fn t1()') },
  });
  assert.equal(code, 0);
});

test('one-token change in a tests/ directory file still fails (Issue #272)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-fn t1() { assert!(true); }',
    '+fn t1() { assert!(false); }',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nfn t1() { assert!(true); }\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nfn t1() { assert!(false); }\n' },
  });
  assert.equal(code, 1);
});

test('non-.rs files are ignored', () => {
  const diff = [
    'diff --git a/docs/foo.md b/docs/foo.md',
    '--- a/docs/foo.md',
    '+++ b/docs/foo.md',
    '@@ -1,1 +1,1 @@',
    '-old',
    '+new',
  ].join('\n');
  const code = main([], { diffText: diff, oldSources: {}, newSources: {} });
  assert.equal(code, 0);
});

test('empty diff passes trivially (nothing changed -> tests untouched)', () => {
  const code = main([], { diffText: '', oldSources: {}, newSources: {} });
  assert.equal(code, 0);
});

test('missing new-side source for a changed .rs file fails closed (exit 2)', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -1,1 +1,1 @@',
    '-pub fn prod_a() {}',
    '+pub fn prod_a() -> u32 { 1 }',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: {},
  });
  assert.equal(code, 2);
});

test('benches/ changes are not flagged (out of test scope, not .rs tests)', () => {
  const diff = [
    'diff --git a/crates/x/benches/b.rs b/crates/x/benches/b.rs',
    '--- a/crates/x/benches/b.rs',
    '+++ b/crates/x/benches/b.rs',
    '@@ -1,1 +1,1 @@',
    '-fn bench_old() {}',
    '+fn bench_new() {}',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/benches/b.rs': 'fn bench_old() {}\n' },
    newSources: { 'crates/x/benches/b.rs': 'fn bench_new() {}\n' },
  });
  assert.equal(code, 0);
});

test('whole-file deletion of a tests/ file is a violation (Issue #231)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    'deleted file mode 100644',
    '--- a/crates/x/tests/integ.rs',
    '+++ /dev/null',
    '@@ -1,2 +0,0 @@',
    '-#[test]',
    '-fn t() {}',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nfn t() {}\n' },
    newSources: {},
  });
  assert.equal(code, 1);
});

test('whole-file deletion of a production file is not flagged (Issue #231)', () => {
  const diff = [
    'diff --git a/crates/x/src/prod.rs b/crates/x/src/prod.rs',
    'deleted file mode 100644',
    '--- a/crates/x/src/prod.rs',
    '+++ /dev/null',
    '@@ -1,1 +0,0 @@',
    '-pub fn p() {}',
  ].join('\n');
  const code = main([], {
    diffText: diff,
    oldSources: { 'crates/x/src/prod.rs': 'pub fn p() {}\n' },
    newSources: {},
  });
  assert.equal(code, 0);
});
