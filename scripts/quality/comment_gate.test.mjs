import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { applyFixes, check, findComments, codeFingerprint, lexRust, lexJs, lexShell, lexToml, lexYaml, stripComments, scopeFiles } from './comment_gate.mjs';

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

test('shell: CRLF heredoc terminator still closes the heredoc', () => {
  const CRLF = '\r\n';
  const src = 'cat <<EOF' + CRLF + 'body' + CRLF + 'EOF' + CRLF + '# real comment' + CRLF;
  const comments = findComments(src, 'shell');
  assert.equal(comments.length, 1);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# real comment\r');
});

test('shell: nested quoting inside $( ) does not desync comment detection', () => {
  const NL2 = '\n';
  const src = 'v="$(printf \'%s\' "$m" | sed -n \'s/x/y/p\')"' + NL2 + '# real comment' + NL2 + 'echo "$v"';
  const comments = findComments(src, 'shell');
  assert.equal(comments.length, 1);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# real comment');
});

test('toml: line and inline comments are found', () => {
  const src = '# header' + NL + 'key = "v" # trailing' + NL + '[table]';
  const comments = findComments(src, 'toml');
  assert.equal(comments.length, 2);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# header');
  assert.equal(src.slice(comments[1].start, comments[1].end), '# trailing');
  const code = lexToml(src).filter((s) => s.type === 'code').map((s) => src.slice(s.start, s.end)).join('');
  assert.ok(code.includes('key = "v"'));
});

test('toml: hashes inside strings are not comments', () => {
  const src = 'url = "https://x/#frag"' + NL + "lit = 'a#b'" + NL + 'multi = """' + NL + '# inside block' + NL + '"""';
  assert.deepEqual(findComments(src, 'toml'), []);
});

test('toml: fingerprint ignores comments and whitespace', () => {
  const withComments = 'a = 1 # one' + NL + '# two' + NL + 'b = 2';
  const bare = 'a = 1' + NL + 'b = 2';
  assert.equal(codeFingerprint(withComments, 'toml'), codeFingerprint(bare, 'toml'));
});

test('toml: unterminated string swallows to EOF without false comments', () => {
  const src = 'a = "unterminated # nope';
  assert.deepEqual(findComments(src, 'toml'), []);
});

test('yaml: line-leading comments are found', () => {
  const src = 'ruleDirs: [rules]' + NL + '# note' + NL + '  # indented';
  const comments = findComments(src, 'yaml');
  assert.equal(comments.length, 2);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# note');
  const code = lexYaml(src).filter((s) => s.type === 'code').map((s) => src.slice(s.start, s.end)).join('');
  assert.ok(code.includes('ruleDirs'));
});

test('yaml: hash inside a plain value line is not flagged', () => {
  const src = 'message: value with # inline';
  assert.deepEqual(findComments(src, 'yaml'), []);
});

test('stripComments drops comment-only lines entirely and trims inline spans', () => {
  const src = '# top' + NL + 'a = 1 # one' + NL + 'b = 2' + NL;
  assert.equal(stripComments(src, 'toml'), 'a = 1' + NL + 'b = 2' + NL);
});

test('stripComments keeps CRLF line endings intact', () => {
  const src = '# top\r\na = 1 # one\r\nb = 2\r\n';
  assert.equal(stripComments(src, 'toml'), 'a = 1\r\nb = 2\r\n');
});

test('stripComments is a no-op on clean sources', () => {
  const src = 'a = 1' + NL + 'b = 2' + NL;
  assert.equal(stripComments(src, 'toml'), src);
});

test('ps1: line and block comments are found, strings are not', () => {
  const src = 'Write-Host "a # b"' + NL + '# note' + NL + '<# block' + NL + 'still block #>' + NL + "Write-Host 'don''t # stop'";
  const comments = findComments(src, 'ps1');
  assert.equal(comments.length, 2);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# note');
  assert.ok(src.slice(comments[1].start, comments[1].end).includes('still block'));
});

test('ps1: here-string content is not a comment', () => {
  const src = '$t = @"' + NL + 'not # a comment' + NL + '"@' + NL + '# real note';
  const comments = findComments(src, 'ps1');
  assert.equal(comments.length, 1);
  assert.equal(src.slice(comments[0].start, comments[0].end), '# real note');
});

test('ps1: fingerprint ignores comments and whitespace', () => {
  const withComments = 'Get-ChildItem # go' + NL + 'Write-Host 1';
  const bare = 'Get-ChildItem' + NL + 'Write-Host 1';
  assert.equal(codeFingerprint(withComments, 'ps1'), codeFingerprint(bare, 'ps1'));
});

test('js: drift-guard marker comments are flagged', () => {
  const src = 'const a = 1;' + NL + '// [done-evidence-begin]' + NL + 'const b = 2;' + NL + '// run-metadata-lookup-end' + NL + '// [x-begin] prose here' + NL + 'const c = 3;';
  const comments = findComments(src, 'js');
  assert.equal(comments.length, 3);
});

test('stripComments removes drift-guard marker lines', () => {
  const src = 'const a = 1;' + NL + '// [done-evidence-begin]' + NL + 'const b = 2;';
  assert.equal(stripComments(src, 'js'), 'const a = 1;' + NL + 'const b = 2;');
});

test('check reports comment line numbers in source order', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'comment-gate-check-'));
  try {
    fs.writeFileSync(path.join(root, 'sample.toml'), 'a = 1' + NL + '# gone' + NL + 'b = 2' + NL + NL + '# also gone' + NL);
    const result = check(root);
    assert.equal(result.ok, false);
    assert.deepEqual(result.findings.map((f) => f.line), [2, 5]);
    assert.ok(result.findings.every((f) => path.basename(f.file) === 'sample.toml'));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('applyFixes strips comments and check with its result verifies clean', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'comment-gate-fix-'));
  try {
    fs.writeFileSync(path.join(root, 'sample.toml'), '# top' + NL + 'a = 1 # one' + NL + 'b = 2' + NL);
    const fix = applyFixes(root);
    assert.equal(fix.filesChanged, 1);
    assert.equal(fix.comments, 2);
    const result = check(root, fix);
    assert.equal(result.ok, true);
    assert.equal(fs.readFileSync(path.join(root, 'sample.toml'), 'utf8'), 'a = 1' + NL + 'b = 2' + NL);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('check prefers precomputed sources over disk contents', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'comment-gate-cache-'));
  try {
    fs.writeFileSync(path.join(root, 'clean.toml'), 'a = 1' + NL);
    const files = scopeFiles(root);
    const sources = new Map(files.map((f) => [f.path, 'x = 1' + NL + '# dirty' + NL]));
    const result = check(root, { files, sources });
    assert.equal(result.ok, false);
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].line, 2);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('scopeFiles covers crates/scripts code plus repo toml and config yml', () => {
  const files = scopeFiles();
  assert.ok(files.length > 150);
  const paths = files.map((f) => f.path.replace(/\\/g, '/'));
  assert.ok(paths.some((p) => p.endsWith('/Cargo.toml')), 'root Cargo.toml');
  assert.ok(paths.some((p) => p.endsWith('/cccc.toml')), 'cccc.toml');
  assert.ok(paths.some((p) => p.endsWith('/sgconfig.yml')), 'sgconfig.yml');
  assert.ok(paths.some((p) => p.includes('/templates/pipelines/')), 'templates pipelines');
  assert.ok(paths.some((p) => p.includes('/data/')), 'data');
  assert.ok(paths.some((p) => p.includes('/.claude/workflows/feature-pipeline.js')), 'workflows js');
  assert.ok(paths.some((p) => p.endsWith('/rss_bench.ps1')), 'rss_bench.ps1');
  assert.ok(paths.some((p) => p.includes('/docs/explainers/')), 'docs explainers');
  assert.ok(!paths.some((p) => p.includes('/ast-grep-tests/')), 'ast-grep-tests excluded');
  assert.ok(!paths.some((p) => p.includes('/references/')), 'references vendored tree excluded');
  assert.ok(!paths.some((p) => p.includes('/.claude/skills/')), 'vendored skills excluded');
  assert.ok(!paths.some((p) => p.includes('/.claude/hooks/')), 'local hooks excluded');
  assert.ok(paths.some((p) => p.includes('/.cargo/')), '.cargo included');
  for (const f of files) {
    assert.ok(['.rs', '.mjs', '.js', '.sh', '.ps1', '.toml', '.yml', '.yaml'].some((e) => f.path.endsWith(e)), f.path);
  }
});
