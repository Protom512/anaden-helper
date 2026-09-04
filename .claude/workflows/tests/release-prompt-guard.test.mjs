// TDD tests for the Release agent prompt empty-release defense (Issue #66 Task 2).
// The Release agent prompt must itself contain a pre-commit assertion:
//   git diff --cached --name-only → zero tracked changes (and no R7 snapshot
//   commit) ⇒ abort push/PR and report ABORTED (double defense with the JS
//   pre-check from Task 1).
// Like team-verdict.test.mjs, we extract the marker-delimited block from
// feature-pipeline.js and evaluate it so tests exercise shipped code.
//
// Issue #152 要件1 (UC-1/UC-2): buildReleasePrompt must interpolate the real
// consensus verdicts (per-lane GO/NO-GO/CONDITIONAL + final verdict +
// CONDITIONAL resolution rationale from judgment_calls). The stale literals
// "(6次元 all GO)" (header) and "6-dimension commit gate: all GO" (PR body)
// are forbidden; "all GO" may only appear when every active lane is GO.
// Lane count comes from ACTIVE_GATE_DIMENSIONS (P-008 docs-only short-circuit
// may drop reliability/performance/extensibility — never hardcode 6).
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
// The block defines buildReleasePrompt(consensus, gateReviews, activeDimensions,
// ticket, snapshotBranch) and buildGateVerdictSection(consensus, gateReviews,
// activeDimensionKeys) (Issue #152). Evaluate and call with representative args.
const api = new Function(`${block}; return { buildReleasePrompt, buildGateVerdictSection };`)();

const ALL_SIX_KEYS = [
  'reliability', 'performance', 'extensibility', 'governance', 'security', 'integration',
];
const dimensionObjects = (keys) => keys.map((key) => ({ key, prompt: `prompt:${key}` }));
const review = (dimension, verdict) => ({
  verdict, dimension, findings: [], summary: `${dimension} summary`,
});
const goConsensus = (extra = {}) => ({
  final_verdict: 'GO',
  commit_message: 'feat(x): do a thing',
  commit_body: 'body text',
  blocking_issues: [],
  judgment_calls: [],
  follow_up_items: [],
  ...extra,
});

const args = [
  goConsensus(),
  ALL_SIX_KEYS.map((k) => review(k, 'GO')),
  dimensionObjects(ALL_SIX_KEYS),
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

test('buildGateVerdictSection is exported (Issue #152)', () => {
  assert.equal(typeof api.buildGateVerdictSection, 'function');
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

// --- Issue #152 要件1: consensus 実 verdict interpolation (UC-1/UC-2) ---

// UC-1 (正常系): 全 lane GO → per-lane verdict 行 + final verdict 行が記載され、
// "all GO" は全 lane GO のため許容される。
test('UC-1: all-GO consensus → per-lane verdict lines + final verdict + all GO allowed', () => {
  const p = api.buildReleasePrompt(...args);
  for (const key of ALL_SIX_KEYS) {
    assert.match(p, new RegExp(`- ${key}: GO`), `per-lane line for ${key}`);
  }
  assert.match(p, /final verdict: GO/);
  assert.match(p, /all GO/u);
});

// UC-2 (正常系・乖離検証): 1 lane CONDITIONAL + final GO → CONDITIONAL lane と
// 解消根拠 (judgment_calls の consensus 文) が記載され、固定 "all GO" は出現しない。
test('UC-2: one CONDITIONAL lane + final GO → CONDITIONAL line + resolution rationale, no "all GO"', () => {
  const conditionalConsensus = goConsensus({
    judgment_calls: [
      {
        topic: 'reliability CONDITIONAL 解消',
        consensus: 'retry-once miss は minor scope の逸脱で workflow 完走に影響なし。GO 判定を維持する。',
      },
    ],
  });
  const reviews = ALL_SIX_KEYS.map((k) => review(k, k === 'reliability' ? 'CONDITIONAL' : 'GO'));
  const p = api.buildReleasePrompt(
    conditionalConsensus, reviews, dimensionObjects(ALL_SIX_KEYS),
    { title: 'T', issueNumber: 152, acceptanceCriteria: ['a'] },
    'feature/cond-branch'
  );
  assert.match(p, /- reliability: CONDITIONAL/);
  assert.match(p, /final verdict: GO/);
  // resolution rationale from judgment_calls.consensus must be present
  assert.match(p, /GO 判定を維持する/u);
  // the stale "all GO" claim must NOT appear (this is the divergence bug fixed)
  assert.ok(!p.includes('all GO'), 'prompt must not claim "all GO" when a lane is CONDITIONAL');
});

// Negative fixtures (stale literals): even in the all-GO case the removed
// hardcoded strings must not reappear — presence-based assertions cannot
// detect stale literals, so pin their absence explicitly.
test('stale literal "(6次元 all GO)" never appears (header replaced by interpolation)', () => {
  assert.ok(!prompt.includes('6次元 all GO'));
});

test('stale literal "6-dimension commit gate: all GO" never appears (PR body replaced by interpolation)', () => {
  assert.ok(!prompt.includes('6-dimension commit gate: all GO'));
});

// Edge 1: docs-only short-circuit (P-008/Issue #95) — ACTIVE_GATE_DIMENSIONS
// drops reliability/performance/extensibility → 3 lanes (governance/security/
// integration). Lane count must come from activeDimensions, never fixed 6.
test('edge: docs-only 3-lane config → 3-lane verdict lines, dropped lanes absent, no fixed "6"', () => {
  const docsOnlyKeys = ['governance', 'security', 'integration'];
  const reviews = docsOnlyKeys.map((k) => review(k, 'GO'));
  const p = api.buildReleasePrompt(
    goConsensus(), reviews, dimensionObjects(docsOnlyKeys),
    { title: 'T', issueNumber: 152, acceptanceCriteria: ['a'] },
    'feature/docs-branch'
  );
  for (const key of docsOnlyKeys) {
    assert.match(p, new RegExp(`- ${key}: GO`));
  }
  // short-circuited lanes must not be listed as reviewed lanes
  assert.ok(!/- reliability: /.test(p));
  assert.ok(!/- performance: /.test(p));
  assert.ok(!/- extensibility: /.test(p));
  assert.match(p, /3 lane/);
  assert.match(p, /all GO \(3\/3/u);
  assert.match(p, /final verdict: GO/);
});

// Edge 2: gateReviews 欠損要素 (null entries) — MISSING lanes are rendered
// truthfully; no crash, no fabricated GO, no "all GO".
test('edge: gateReviews with missing entries → MISSING rendered, no crash, no "all GO"', () => {
  const reviews = ALL_SIX_KEYS.map((k, i) => (i === 1 || i === 4 ? null : review(k, 'GO')));
  const p = api.buildReleasePrompt(
    goConsensus(), reviews, dimensionObjects(ALL_SIX_KEYS),
    { title: 'T', issueNumber: 152, acceptanceCriteria: ['a'] },
    'feature/missing-branch'
  );
  assert.match(p, /- performance: MISSING/);
  assert.match(p, /- security: MISSING/);
  assert.match(p, /- reliability: GO/);
  assert.ok(!p.includes('all GO'), 'missing lanes must not be reported as all GO');
});

// buildGateVerdictSection unit tests (pure function, Issue #152).

test('buildGateVerdictSection: NO-GO lane rendered; final verdict from consensus', () => {
  const reviews = ALL_SIX_KEYS.map((k) => review(k, k === 'security' ? 'NO-GO' : 'GO'));
  const section = api.buildGateVerdictSection(
    { final_verdict: 'NO-GO', judgment_calls: [] },
    reviews,
    ALL_SIX_KEYS
  );
  assert.match(section, /- security: NO-GO/);
  assert.match(section, /- integration: GO/);
  assert.match(section, /final verdict: NO-GO/);
  assert.ok(!section.includes('all GO'));
});

test('buildGateVerdictSection: CONDITIONAL with empty judgment_calls → explicit fallback note', () => {
  const reviews = ALL_SIX_KEYS.map((k) => review(k, k === 'reliability' ? 'CONDITIONAL' : 'GO'));
  const section = api.buildGateVerdictSection(
    { final_verdict: 'GO', judgment_calls: [] },
    reviews,
    ALL_SIX_KEYS
  );
  assert.match(section, /- reliability: CONDITIONAL/);
  assert.match(section, /judgment_calls/u);
  assert.ok(!section.includes('all GO'));
});

test('buildGateVerdictSection: empty/undefined inputs fail closed without throwing', () => {
  const section = api.buildGateVerdictSection(null, null, []);
  assert.match(section, /final verdict: UNKNOWN/);
  assert.ok(!section.includes('all GO'));
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
