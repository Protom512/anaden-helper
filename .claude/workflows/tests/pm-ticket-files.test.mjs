// TDD tests for Issue #99 Task 1: pm:create-ticket agent の schema に declared
// files (files: string[]) を追加し、PM プロンプトに ticket-declared files の
// 明記を要求する。継続系 ("next") 指示時は PM プロンプトを分岐し、未マージ
// ブランチ・open PR の残作業サマリ形式でチケット生成させる (UC-3)。
//
// Source-scan pattern (workflow script is not importable):
//   Run: node --test .claude/workflows/tests/pm-ticket-files.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

function pmPromptBlock() {
  const at = src.indexOf('pm:create-ticket');
  assert.ok(at > -1, 'pm:create-ticket agent call exists');
  // prompt precedes the options object; slice a generous window backwards
  const promptStart = src.lastIndexOf('`You are the Product Manager', at);
  assert.ok(promptStart > -1, 'PM prompt template literal found');
  return src.slice(promptStart, at + 4000);
}

test('pm:create-ticket schema includes files: array of strings', () => {
  const at = src.indexOf('pm:create-ticket');
  const schemaStart = src.indexOf('schema: {', at);
  assert.ok(schemaStart > -1, 'schema declared');
  const schemaEnd = src.indexOf('})', schemaStart);
  const schema = src.slice(schemaStart, schemaEnd);
  assert.match(schema, /files:\s*\{\s*type:\s*'array',\s*items:\s*\{\s*type:\s*'string'\s*\}\s*\}/,
    'files: { type: array, items: string } present in schema');
});

test('PM prompt requires declaring ticket files (ticket-declared files)', () => {
  const block = pmPromptBlock();
  assert.match(block, /files/, 'prompt mentions files');
  assert.match(block, /ticket[- ]declared|宣言|明記/, 'prompt demands explicit file declaration');
  assert.match(block, /files\s*\(|files:|files フィールド|StructuredOutput.*files|files.*配列/,
    'prompt explains files must be returned as an array');
});

test('PM prompt branches for continuation ("next") instructions (UC-3)', () => {
  const block = pmPromptBlock();
  assert.ok(block.includes('${isBacklogPick'), 'PM prompt branches on isBacklogPick');
  assert.match(block, /未マージ|unmerged/i, 'mentions unmerged branches / open PRs');
  assert.match(block, /open PR|PR/, 'mentions open PR');
  assert.match(block, /残作業|remaining work/i, 'mentions remaining work summary');
  assert.match(block, /対象ブランチ|branch/, 'mentions target branch');
  assert.match(block, /未了工程|unfinished|残り工程/, 'mentions unfinished steps');
  assert.match(block, /受け入れ基準|acceptance criteria/, 'mentions acceptance criteria');
});

test('continuation branch is distinct from new-feature template (no mechanical template application)', () => {
  const block = pmPromptBlock();
  // The branch must instruct a remaining-work-summary ticket format rather than
  // blindly reusing the new-feature template (persona/motivation/use cases).
  const branchAt = block.indexOf('${isBacklogPick');
  const ternary = block.slice(branchAt, branchAt + 3000);
  assert.match(ternary, /サマリ形式|summary format/i, 'branch defines summary format');
  assert.ok(!/:\s*`.*Use the GitHub Issue template.*persona.*remaining work/s.test(ternary) === false
    || /残作業サマリ/.test(ternary),
    'branch is a genuine alternative format');
});

// ── Issue #104: ticketKind (commit-range fallback の誤検出防止) ──
test('pm:create-ticket schema requires ticketKind enum (Issue #104)', () => {
  const begin = src.indexOf('// [pm-ticket-files-begin]');
  const end = src.indexOf('// [pm-ticket-files-end]');
  assert.ok(begin >= 0 && end > begin);
  const block = src.slice(begin, end);
  assert.match(block, /ticketKind:\s*\{\s*type:\s*'string',\s*enum:\s*\['new-implementation',\s*'continuation'\]/,
    'schema has ticketKind enum');
  assert.match(block, /required:\s*\['title',\s*'priority',\s*'summary',\s*'files',\s*'ticketKind'\]/,
    'ticketKind is required');
});

test('commit-range fallback gated to continuation tickets only (Issue #104)', () => {
  const fiAt = src.indexOf('const precheckChangedFiles');
  assert.ok(fiAt > 0);
  const block = src.slice(fiAt - 600, fiAt + 500);
  assert.match(block, /precheckTicketKind === 'continuation' \? \[\.\.\.new Set\(precheckRangeFiles\)\] : \[\]/,
    'fallback applies only when ticketKind === continuation');
});

test('PM prompt explains ticketKind semantics (Issue #104)', () => {
  const begin = src.indexOf('// [pm-ticket-files-begin]');
  const end = src.indexOf('// [pm-ticket-files-end]');
  const block = src.slice(begin, end);
  assert.match(block, /"continuation":\s*未マージブランチ\/open PR の残作業/, 'continuation explained');
  assert.match(block, /"new-implementation":\s*ゼロから新規実装/, 'new-implementation explained');
});
