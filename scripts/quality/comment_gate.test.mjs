import { test } from 'node:test';
import assert from 'node:assert/strict';
import { findComments, codeFingerprint, lexRust, lexJs, lexShell, scopeFiles } from './comment_gate.mjs';

const NL = '\n';

test('rust: line and doc comments are found', () => {
  const src = 'let a = 1;' + NL + '    // note' + NL + '    /// doc' + NL + '    //! inner' + NL + 'let b = 2;';
  const comments = findComments(src, 'rust');
  assert.equal(comments.length, 3);
  assert.equal(src.slice(comments[0].start, comments[0].end), '// note');
  assert.equal(src.slice(comments[1].start, comments[1].end), '/// doc');
  assert.equal(src.slice(comments[2].start, comments[2].end), '//! inner');
});

test('rust: nested block comments count as one span', () => {
  const src = 'let a = 1; /* outer /* inner */ still outer */ let b = 2;';
  const comments = findComments(src, 'rust');
  assert.equal(comments.length, 1);
  assert.ok(src.slice(comments[0].start, comments[0].end).includes('inner'));
});

test('rust: comments inside strings are not comments', () => {
  const src = 'let s = "http://x/*y*/"; let r = r#"has // and /* inside"#; let b = br#"//"#;';
  assert.deepEqual(findComments(src, 'rust'), []);
});

test('rust: char literals and lifetimes are distinguished', () => {
  const src = "let c = 'a'; let n = '\\n'; let u = '\\u{1F600}'; fn f<'a>(x: &'a str) -> &'a str { x } let q = '\\'';";
  assert.deepEqual(findComments(src, 'rust'), []);
});

test('rust: fingerprint ignores comments and whitespace', () => {
  const withComments = 'let a = 1; // one' + NL + '/* two */ let b = 2;';
  const bare = 'let a = 1;' + NL + 'let b = 2;';
  assert.equal(codeFingerprint(withComments, 'rust'), codeFingerprint(bare, 'rust'));
});

test('js: line and block comments are found', () => {
  const src = 'const a = 1;' + NL + '// note' + NL + '/* block */ const b = 2;';
  const comments = findComments(src, 'js');
  assert.equal(comments.length, 2);
  assert.equal(src.slice(comments[0].start, comments[0].end), '// note');
  assert.equal(src.slice(comments[1].start, comments[1].end), '/* block */');
});

test('js: strings containing slashes are not comments', () => {
  const src = 'const u = "http://example.com"; const c = `template // text`;';
  assert.deepEqual(findComments(src, 'js'), []);
});

test('js: regex literal containing slashes is code, division stays division', () => {
  const src = 'const re = /a\\/b\\/c/; const d = 10 / 2 / 5; const e = x / y;';
  assert.deepEqual(findComments(src, 'js'), []);
  const segs = lexJs(src);
  const code = segs.filter((s) => s.type === 'code').map((s) => src.slice(s.start, s.end)).join('');
  assert.ok(code.includes('/a\\/b\\/c/'));
  assert.ok(code.includes('10 / 2 / 5'));
});

test('js: regex after keyword and after operators', () => {
  const src = 'return /^[a-z]+$/i.test(s); const y = (x || /ab/).source;';
  assert.deepEqual(findComments(src, 'js'), []);
});

test('js: comments inside template expressions are found', () => {
  const src = 'const t = `${ /* c */ value }`;';
  const comments = findComments(src, 'js');
  assert.equal(comments.length, 1);
  assert.equal(src.slice(comments[0].start, comments[0].end), '/* c */');
});

test('js: fingerprint ignores comments and whitespace', () => {
  const withComments = 'const a = 1; // one' + NL + 'const b = 2;';
  const bare = 'const a = 1;' + NL + 'const b = 2;';
  assert.equal(codeFingerprint(withComments, 'js'), codeFingerprint(bare, 'js'));
});

test('shell: hash comments at line start and after whitespace', () => {
  const src = 'echo hi' + NL + '# comment' + NL + '  # spaced' + NL + 'echo ${v#prefix} bye';
  const comments = findComments(src, 'shell');
  assert.equal(comments.length, 2);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# comment');
});

test('shell: shebang is exempt, hashes in strings are not comments', () => {
  const src = '#!/bin/bash' + NL + 'echo "a # b" \'c # d\'' + NL + 'echo done';
  assert.deepEqual(findComments(src, 'shell'), []);
});

test('shell: heredoc body is not scanned for comments', () => {
  const src = 'cat <<EOF' + NL + 'not # a comment' + NL + 'EOF' + NL + '# real comment' + NL + 'cat <<-\'TAB\'' + NL + '\tbody # nope' + NL + '\tTAB' + NL + 'echo end';
  const comments = findComments(src, 'shell');
  assert.equal(comments.length, 1);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# real comment');
});

test('scopeFiles covers crates and scripts code extensions only', () => {
  const files = scopeFiles();
  assert.ok(files.length > 100);
  for (const f of files) {
    assert.ok(f.path.includes('crates') || f.path.includes('scripts'), f.path);
    assert.ok(['.rs', '.mjs', '.sh'].some((e) => f.path.endsWith(e)), f.path);
  }
});
