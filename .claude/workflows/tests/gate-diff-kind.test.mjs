import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyDiffKind, isDocsPath } from '../gate-diff-kind.js';

test('markdown-only diff -> docs-only', () => {
  assert.equal(classifyDiffKind(['README.md', 'docs/guide.md']), 'docs-only');
});

test('.claude/rules paths -> docs-only', () => {
  assert.equal(
    classifyDiffKind(['.claude/rules/pipeline-evidence-verification.md']),
    'docs-only',
  );
  assert.equal(
    classifyDiffKind(['.claude/rules/new-rule.md', 'CLAUDE.md']),
    'docs-only',
  );
});

test('docs/ tree (non-md files too) -> docs-only', () => {
  assert.equal(classifyDiffKind(['docs/adr/0001.txt', 'README.md']), 'docs-only');
});

test('nested .md anywhere -> docs-only', () => {
  assert.equal(classifyDiffKind(['crates/foo/README.md']), 'docs-only');
});

test('rust source diff -> code', () => {
  assert.equal(classifyDiffKind(['crates/foo/src/lib.rs']), 'code');
});

test('config/build files -> code (fail-closed to code)', () => {
  assert.equal(classifyDiffKind(['Cargo.toml']), 'code');
  assert.equal(classifyDiffKind(['package.json', 'run.js']), 'code');
});

test('docs + code -> mixed', () => {
  assert.equal(
    classifyDiffKind(['README.md', 'crates/foo/src/lib.rs']),
    'mixed',
  );
  assert.equal(
    classifyDiffKind(['docs/a.md', '.claude/workflows/feature-pipeline.js']),
    'mixed',
  );
});

test('empty array -> code (fail-closed)', () => {
  assert.equal(classifyDiffKind([]), 'code');
});

test('null/undefined/non-array input -> code (fail-closed)', () => {
  assert.equal(classifyDiffKind(null), 'code');
  assert.equal(classifyDiffKind(undefined), 'code');
  assert.equal(classifyDiffKind('README.md'), 'code');
});

test('blank/whitespace/non-string entries -> code (fail-closed)', () => {
  assert.equal(classifyDiffKind(['', '   ']), 'code');
  assert.equal(classifyDiffKind(['README.md', null, 42]), 'code');
});

test('unknown extension -> code (fail-closed)', () => {
  assert.equal(classifyDiffKind(['data/schema.json']), 'code');
  assert.equal(classifyDiffKind(['image.png']), 'code');
});

test('isDocsPath basic contract', () => {
  assert.equal(isDocsPath('a/b.md'), true);
  assert.equal(isDocsPath('.claude/rules/x.md'), true);
  assert.equal(isDocsPath('docs/anything.txt'), true);
  assert.equal(isDocsPath('src/main.rs'), false);
  assert.equal(isDocsPath(''), false);
  assert.equal(isDocsPath(null), false);
});
