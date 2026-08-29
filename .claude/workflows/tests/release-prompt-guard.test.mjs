// TDD tests for the Release agent prompt empty-release defense (Issue #66 Task 2).
// The Release agent prompt must itself contain a pre-commit assertion:
//   git diff --cached --name-only → zero tracked changes (and no R7 snapshot
//   commit) ⇒ abort push/PR and report ABORTED (double defense with the JS
//   pre-check from Task 1).
// Like team-verdict.test.mjs, we extract the marker-delimited block from
// feature-pipeline.js and evaluate it so tests exercise shipped code.
//
// Run: node --test .claude/workflows/tests/release-prompt-guard.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [release-prompt-begin]';
const END = '// [release-prompt-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);
assert.ok(begin >= 0, 'feature-pipeline.js contains release-prompt-begin marker');
assert.ok(end > begin, 'feature-pipeline.js contains release-prompt-end marker after begin');

const block = src.slice(begin + BEGIN.length, end);
// The block defines buildReleasePrompt(consensus, ticket, snapshotBranch) as a
// template-literal builder. Evaluate and call it with representative args.
const api = new Function(`${block}; return { buildReleasePrompt };`)();

const args = [
  { commit_message: 'feat(x): do a thing', commit_body: 'body text' },
  { title: 'T', issueNumber: 66, acceptanceCriteria: ['a', 'b'] },
  'feature/test-branch',
];
let prompt = null;
if (typeof api.buildReleasePrompt === 'function') {
  prompt = api.buildReleasePrompt(...args);
}

test('buildReleasePrompt is exported and returns a non-empty string', () => {
  assert.equal(typeof api.buildReleasePrompt, 'function');
  assert.ok(typeof prompt === 'string' && prompt.length > 200);
});

test('prompt instructs checking git diff --cached --name-only right before commit', () => {
  assert.match(prompt, /git diff --cached --name-only/);
});

test('prompt instructs ABORT (skip push/PR) when tracked changes are zero', () => {
  assert.match(prompt, /ABORTED/);
  // abort must happen BEFORE push and PR creation
  const abortIdx = prompt.indexOf('ABORTED');
  const pushIdx = prompt.search(/git push/);
  assert.ok(abortIdx >= 0 && pushIdx > abortIdx, 'ABORTED instruction precedes push instructions');
});

test('prompt keeps R7 snapshot-commit exception (snapshot-only diff is a normal path)', () => {
  assert.match(prompt, /snapshot commit/u);
  // snapshot branch name interpolated
  assert.match(prompt, /feature\/test-branch/);
});

test('prompt handles exclusion-pattern-only staging as zero (excluded files are not real changes)', () => {
  assert.match(prompt, /\.claude\.old|除外/u);
});

test('prompt requires reporting the current branch and diff summary on abort (resumable)', () => {
  assert.match(prompt, /branch|ブランチ/u);
});
