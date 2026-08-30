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

// --- Issue #109 Task 3: conventional-commit verbatim preservation ---
// The release commit step must use the gate-agreed commit message verbatim
// (only scope edits allowed). Dropping the body/Co-Authored-By or rewriting
// the type (e.g. feat → feat(snapshot)) is forbidden.

test('prompt requires the gate-agreed commit message to be used verbatim (scope edits only)', () => {
  assert.match(prompt, /verbatim/i);
  assert.match(prompt, /scope のみ|scope only/iu);
});

test('prompt forbids dropping the commit body and Co-Authored-By trailer', () => {
  assert.match(prompt, /Co-Authored-By.*落と|body.*落と|削除禁止|保持/u);
  assert.match(prompt, /Co-Authored-By/u);
});

test('prompt forbids rewriting the commit type (e.g. feat → feat(snapshot))', () => {
  assert.match(prompt, /型.*書き換え.*禁止|type.*rewrite.*forbid|書き換え禁止/u);
});

test('R7 snapshot amend instruction preserves the original conventional-commit message (scope fix only)', () => {
  assert.match(prompt, /amend/u);
  assert.match(prompt, /amend.*[^\n]*scope|scope.*[^\n]*amend/us);
  assert.match(prompt, /元.*conventional-commit.*メッセージ.*保持|既存のコミットメッセージ.*保持/us);
});

// release-pipeline.js Phase 4 Commit prompt must also instruct verbatim use.
const rpPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'release-pipeline.js');
const rpSrc = readFileSync(rpPath, 'utf8');
const phase4 = rpSrc.slice(rpSrc.indexOf('// Phase 4: Commit'), rpSrc.indexOf('// Phase 5'));

test('release-pipeline.js Phase 4 commit prompt instructs original message verbatim (scope edit only)', () => {
  assert.ok(phase4.length > 0, 'Phase 4 block exists in release-pipeline.js');
  assert.match(phase4, /verbatim/i);
  assert.match(phase4, /scope のみ|scope only/iu);
  assert.match(phase4, /Co-Authored-By/u);
  assert.match(phase4, /保持/u);
});
