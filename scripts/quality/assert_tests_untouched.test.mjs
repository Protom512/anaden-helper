import test from 'node:test';
import assert from 'node:assert/strict';
import { main } from './assert_tests_untouched.mjs';

const OLD_SRC = [
  'pub fn prod_a() {}',
  'pub fn prod_b() {}',
  '',
  '#[cfg(test)]',
  'mod tests {',
  '    #[test]',
  '    fn t1() { assert!(true); }',
  '}',
].join('\n');

const NEW_SRC_PROD_ONLY = OLD_SRC.replace('pub fn prod_a() {}', 'pub fn prod_a() -> u32 { 1 }');

const NEW_SRC_DELETED_PROD = ['pub fn prod_b() {}', '', '#[cfg(test)]', 'mod tests {', '    #[test]', '    fn t1() { assert!(true); }', '}'].join('\n');

const call = (opts) => main([], { allowlist: [], ...opts });


test('added line inside a new-side test region is a violation (exit 1)', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -7,1 +7,1 @@',
    '-    fn t1() { assert!(true); }',
    '+    fn t1() { assert!(false); }',
  ].join('\n');
  const code = call({
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
  const newSrc = ['#cfg', ''].join('\n');
  const code = call({
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
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': NEW_SRC_PROD_ONLY },
  });
  assert.equal(code, 0);
});

test('deleting production lines (test region untouched) passes', () => {
  const diff = [
    'diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs',
    '--- a/crates/x/src/lib.rs',
    '+++ b/crates/x/src/lib.rs',
    '@@ -1,2 +1,1 @@',
    '-pub fn prod_a() {}',
    ' pub fn prod_b() {}',
  ].join('\n');
  const code = call({
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
  const code = call({
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
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/src/lib.rs': OLD_SRC },
    newSources: { 'crates/x/src/lib.rs': newSrc },
  });
  assert.equal(code, 1);
});

test('brand-new file under tests/ passes: additive coverage is allowed (Issue #272)', () => {
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
  const code = call({
    diffText: diff,
    oldSources: {},
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nfn new_test() {}\n' },
  });
  assert.equal(code, 0);
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
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(\n    j.point(),\n    (1, 2)\n);\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(j.point(), (1, 2));\n' },
  });
  assert.equal(code, 0);
});

test('rustfmt array collapse that drops a trailing comma is format-only (Issue #272 v2)', () => {
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
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nfor name in [\n    "A",\n    "B",\n] {}\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nfor name in ["A", "B"] {}\n' },
  });
  assert.equal(code, 0);
});

test('rustfmt argument explode that inserts a trailing comma is format-only (Issue #272 v2)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,8 @@',
    '-let j = with_config(9, Config { a: 0, b: 0.15 });',
    '+let j = with_config(',
    '+    9,',
    '+    Config {',
    '+        a: 0,',
    '+        b: 0.15,',
    '+    },',
    '+);',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': 'let j = with_config(9, Config { a: 0, b: 0.15 });\n' },
    newSources: {
      'crates/x/tests/integ.rs': 'let j = with_config(\n    9,\n    Config {\n        a: 0,\n        b: 0.15,\n    },\n);\n',
    },
  });
  assert.equal(code, 0);
});

test('whitespace change inside a string literal fails (Issue #272 v2 hole)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-assert_eq!(label(), "key zero");',
    '+assert_eq!(label(), "keyzero");',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(label(), "key zero");\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nassert_eq!(label(), "keyzero");\n' },
  });
  assert.equal(code, 1);
});

test('content change inside a raw string fails (Issue #272 v2)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-let s = r#"key zero"#;',
    '+let s = r#"keyzero"#;',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nlet s = r#"key zero"#;\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nlet s = r#"keyzero"#;\n' },
  });
  assert.equal(code, 1);
});

test('comment-only whitespace reflow passes (Issue #272 v2)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-// checks   the   zero   case',
    '+// checks the zero case',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\n// checks   the   zero   case\nassert!(true);\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\n// checks the zero case\nassert!(true);\n' },
  });
  assert.equal(code, 0);
});

test('comma moved to a non-trailing position fails (Issue #272 v2)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-let v = [1, 2];',
    '+let v = [1 2,];',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nlet v = [1, 2];\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nlet v = [1 2,];\n' },
  });
  assert.equal(code, 1);
});

test('comma removed from inside a list fails (Issue #272 v2)', () => {
  const diff = [
    'diff --git a/crates/x/tests/integ.rs b/crates/x/tests/integ.rs',
    '--- a/crates/x/tests/integ.rs',
    '+++ b/crates/x/tests/integ.rs',
    '@@ -2,1 +2,1 @@',
    '-run(a, b);',
    '+run(a b);',
  ].join('\n');
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/tests/integ.rs': '#[test]\nrun(a, b);\n' },
    newSources: { 'crates/x/tests/integ.rs': '#[test]\nrun(a b);\n' },
  });
  assert.equal(code, 1);
});

test('violation in an allowlisted file passes (Issue #272 v2)', () => {
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
    allowlist: [{ file: 'crates/x/tests/integ.rs', reason: 'mechanical fix, reviewed', ref: '#272' }],
  });
  assert.equal(code, 0);
});

test('stale allowlist entry fails (Issue #272 v2)', () => {
  const code = main([], {
    diffText: '',
    oldSources: {},
    newSources: {},
    allowlist: [{ file: 'crates/x/tests/other.rs', reason: 'mechanical fix, reviewed', ref: '#272' }],
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
  const code = call({
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
  const code = call({
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
  const code = call({ diffText: diff, oldSources: {}, newSources: {} });
  assert.equal(code, 0);
});

test('empty diff passes trivially (nothing changed -> tests untouched)', () => {
  const code = call({ diffText: '', oldSources: {}, newSources: {} });
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
  const code = call({
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
  const code = call({
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
  const code = call({
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
  const code = call({
    diffText: diff,
    oldSources: { 'crates/x/src/prod.rs': 'pub fn p() {}\n' },
    newSources: {},
  });
  assert.equal(code, 0);
});
