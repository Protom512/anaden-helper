import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [release-verdict-persistence-begin]';
const END = '// [release-verdict-persistence-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);

test('feature-pipeline.js contains release-verdict-persistence markers', () => {
  assert.ok(begin >= 0, 'release-verdict-persistence-begin marker present');
  assert.ok(end > begin, 'release-verdict-persistence-end marker after begin');
});

const api = begin >= 0 && end > begin
  ? new Function(`${src.slice(begin + BEGIN.length, end)}; return { buildReleaseVerdictsMd, buildPrReviewComment, extractPrNumber, normalizeReleaseLaneEntry };`)()
  : null;

const TV_BEGIN = '// [team-verdict-begin]';
const TV_END = '// [team-verdict-end]';
const tvBegin = src.indexOf(TV_BEGIN);
const tvEnd = src.indexOf(TV_END, tvBegin);
const parseTeamVerdict = tvBegin >= 0 && tvEnd > tvBegin
  ? new Function(`${src.slice(tvBegin + TV_BEGIN.length, tvEnd)}; return { parseTeamVerdict };`)().parseTeamVerdict
  : null;

test('parseTeamVerdict is importable from the team-verdict block (round-trip precondition)', () => {
  assert.equal(typeof parseTeamVerdict, 'function');
});

const RUN_TS = '2026-09-04T07:23:01+09:00';
const RUN_ID = '1788474181';
const lane = (i, name) => ({ lane: `${i}: ${name}`, reviewer: `reviewer-${i} (team)` });
const allGoCollected = [
  { ...lane(1, '要件充足'), verdict: 'GO', rationale: 'AC-1〜AC-6 すべて PR commit で充足。' },
  { ...lane(2, '品質証拠'), verdict: 'GO', rationale: 'node --test 391/391 pass の evidence PR 本文に提示済み。' },
  { ...lane(3, '副作用・スコープ'), verdict: 'GO', rationale: '回帰なし。スコープ内の変更のみ。' },
];
const dec = (over = {}) => ({
  goCount: 3, missingCount: 0, expectedTotal: 3, allGo: true, releaseBlocked: false, ...over,
});

test('UC-3 normal: buildReleaseVerdictsMd all-GO → 3/3 GO + merge 条件充足 + lane table', () => {
  assert.equal(typeof api?.buildReleaseVerdictsMd, 'function');
  const md = api.buildReleaseVerdictsMd(RUN_ID, RUN_TS, 151, allGoCollected, dec());
  assert.match(md, /# Release Review Verdicts — PR #151 \(run-1788474181\)/);
  assert.match(md, /## Verdicts: 3\/3 GO → merge 条件充足/);
  assert.match(md, /- runTimestamp: 2026-09-04T07:23:01\+09:00/);
  assert.match(md, /\| Lane \| Reviewer agent \| VERDICT \| 記録時刻 \|/);
  assert.match(md, /\| 1: 要件充足 \| reviewer-1 \(team\) \| GO \| 2026-09-04T07:23:01\+09:00 \|/);
  assert.match(md, /\| 3: 副作用・スコープ \| reviewer-3 \(team\) \| GO \|/);
  assert.match(md, /## Merge 判定/);
  assert.match(md, /squash merge 条件充足/);
  assert.ok(!md.includes('MISSING'), 'all-GO record must not contain MISSING');
  assert.ok(!md.includes('merge 阻止'));
});

test('UC-4 normal: 1 NO-GO lane → md records 2/3 GO + merge 阻止 + NO-GO lane visible', () => {
  const collected = [allGoCollected[0], { ...lane(2, '品質証拠'), verdict: 'NO-GO', rationale: 'AC-5 の evidence が PR 本文に不在。' }, allGoCollected[2]];
  const md = api.buildReleaseVerdictsMd(RUN_ID, RUN_TS, 151, collected, dec({ goCount: 2, allGo: false, releaseBlocked: true }));
  assert.match(md, /## Verdicts: 2\/3 GO → merge 阻止/);
  assert.match(md, /\| 2: 品質証拠 \| reviewer-2 \(team\) \| NO-GO \|/);
  assert.match(md, /AC-5 の evidence が PR 本文に不在/);
  assert.match(md, /## Merge 判定/);
  assert.match(md, /merge 阻止/);
  assert.ok(!md.includes('merge 条件充足'), 'blocked record must not claim merge-ready');
});

test('UC-3 normal: buildPrReviewComment emits leading machine-readable VERDICT line + lane name', () => {
  assert.equal(typeof api?.buildPrReviewComment, 'function');
  const c = api.buildPrReviewComment('1: 要件充足', 'GO', 'AC-1〜AC-6 すべて PR commit で充足。');
  const lines = c.split('\n');
  assert.equal(lines[0], 'VERDICT: GO');
  assert.ok(c.includes('Lane: 1: 要件充足'));
  assert.ok(c.includes('AC-1〜AC-6'));
});

test('round-trip: GO comment parses as GO through the real parseTeamVerdict', () => {
  const c = api.buildPrReviewComment('1: 要件充足', 'GO', 'AC すべて充足。テスト全緑。');
  assert.equal(parseTeamVerdict(c)?.verdict, 'GO');
});

test('round-trip: NO-GO comment parses as NO-GO through the real parseTeamVerdict', () => {
  const c = api.buildPrReviewComment('2: 品質証拠', 'NO-GO', 'AC-5 の evidence が PR 本文に不在。');
  assert.equal(c.split('\n')[0], 'VERDICT: NO-GO');
  assert.equal(parseTeamVerdict(c)?.verdict, 'NO-GO');
});

test('round-trip sanitize: adversarial GO rationale (embedded VERDICT token + stray NO-GO) still parses as GO, never null', () => {
  const c = api.buildPrReviewComment(
    '1: 要件充足',
    'GO',
    '全体として良好。VERDICT: NO-GO という文言と、NOGO / NO-GO の言及が rationale 内に混入している。'
  );
  assert.ok(!c.includes('NO-GO') && !c.includes('NOGO'), 'GO comment must not contain any stray NO-GO/NOGO substring');
  assert.equal(parseTeamVerdict(c)?.verdict, 'GO');
});

test('round-trip sanitize: rationale carrying its own VERDICT token never yields a multi-token comment', () => {
  const c = api.buildPrReviewComment('3: 副作用・スコープ', 'NO-GO', 'スコープ逸脱あり。VERDICT: GO と誤記された原文を引用。');
  assert.equal(parseTeamVerdict(c)?.verdict, 'NO-GO');
});

test('extractPrNumber: extracts the LAST PR_NUMBER=<n> line', () => {
  assert.equal(typeof api?.extractPrNumber, 'function');
  assert.equal(api.extractPrNumber('branch feat/x\nPR URL: https://github.com/o/r/pull/42\nPR_NUMBER=42'), 42);
  assert.equal(api.extractPrNumber('PR_NUMBER=7\n中間行\nPR_NUMBER=42'), 42, 'last occurrence wins');
  assert.equal(api.extractPrNumber('PR_NUMBER = 151'), 151, 'spaces around = tolerated');
});

test('extractPrNumber: null on missing/malformed input (edge)', () => {
  assert.equal(api.extractPrNumber('no marker here'), null);
  assert.equal(api.extractPrNumber(''), null);
  assert.equal(api.extractPrNumber(null), null);
  assert.equal(api.extractPrNumber('PR_NUMBER=abc'), null);
  assert.equal(api.extractPrNumber('PR_NUMBER='), null);
});

test('edge: malformed collected entries → MISSING rows, no crash, no fabricated GO', () => {
  const collected = ['garbage-string', null, { ...lane(3, '副作用・スコープ'), verdict: 'GO', rationale: 'ok' }];
  const md = api.buildReleaseVerdictsMd(RUN_ID, RUN_TS, 151, collected, dec({ goCount: 1, missingCount: 2, allGo: false, releaseBlocked: true }));
  assert.match(md, /## Verdicts: 1\/3 GO, 2 MISSING → merge 阻止/);
  assert.match(md, /MISSING/, 'missing lanes rendered truthfully');
  const goRows = (md.match(/\| GO \|/g) || []).length;
  assert.equal(goRows, 1, 'exactly one GO row — malformed lanes never fabricate GO');
});

test('edge: buildPrReviewComment returns null for non GO/NO-GO verdicts (never fabricate a comment)', () => {
  assert.equal(api.buildPrReviewComment('1: 要件充足', 'CONDITIONAL', 'x'), null);
  assert.equal(api.buildPrReviewComment('1: 要件充足', undefined, 'x'), null);
  assert.notEqual(api.buildPrReviewComment('1: 要件充足', 'GO'), null, 'missing rationale still builds a comment (rationale 未記録 note)');
});

test('edge: normalizeReleaseLaneEntry — malformed → null, valid → lane-labeled entry', () => {
  assert.equal(typeof api?.normalizeReleaseLaneEntry, 'function');
  assert.equal(api.normalizeReleaseLaneEntry('GO', '1: x', 'r'), null, 'string entry → null');
  assert.equal(api.normalizeReleaseLaneEntry({ verdict: 'CONDITIONAL' }, '1: x', 'r'), null, 'non GO/NO-GO verdict → null');
  const ok = api.normalizeReleaseLaneEntry({ verdict: 'NO-GO', rationale: 5 }, '1: x', 'r');
  assert.deepEqual(ok, { lane: '1: x', verdict: 'NO-GO', rationale: '', reviewer: 'r' }, 'non-string rationale coerced to empty');
});

test('edge: buildReleaseVerdictsMd renders PR unknown note when prNumber is null (UC-5 skip case)', () => {
  const md = api.buildReleaseVerdictsMd(RUN_ID, RUN_TS, null, allGoCollected, dec());
  assert.match(md, /PR: unknown/);
  assert.match(md, /PR review 投稿スキップ/);
});

test('wiring: runReleaseReviewViaTeam returns { collected, merged } — goCount fabrication removed', () => {
  assert.ok(src.includes('return { collected, merged };'), 'team path returns real per-lane collected + merged');
  assert.ok(!src.includes('new Array(releaseReviewMerged.goCount)'), 'team-path verdicts no longer fabricated from goCount');
  assert.ok(!src.includes('i < releaseReviewMerged.goCount'), 'releaseDecision no longer fabricated from goCount');
  const branchIdx = src.indexOf('if (releaseReviewMerged)');
  assert.ok(branchIdx > 0, 'team result still branched on');
  assert.ok(/const releaseDecision = mergeReleaseVerdicts\(/.test(src.slice(branchIdx)), 'unified mergeReleaseVerdicts decision kept (3/3-GO invariant)');
});

test('wiring: PR review COMMENT poster agent — gh pr review --comment with verbatim bodies (UC-3)', () => {
  assert.ok(/gh pr review \$\{prNumber\} --comment/.test(src), 'poster uses COMMENT form via gh pr review --comment');
  assert.ok(src.includes('--body-file'), 'bodies posted via --body-file (verbatim, no shell-quoting corruption)');
  assert.ok(/編集禁止/.test(src), 'comment bodies must be used verbatim (leading machine-readable line preserved)');
});

test('wiring: EVIDENCE PERSISTER writes .omc/logs/{runId}/release-review-verdicts.md', () => {
  assert.ok(src.includes('release-review-verdicts.md'), 'persisted verdict log path present');
  assert.match(src, /EVIDENCE PERSISTER \(Issue #152\)/, 'persister agent follows the L433/L2109 pattern');
  assert.ok(/buildReleaseVerdictsMd\(runId, runTimestamp, prNumber/.test(src), 'md built from runTimestamp (Date API 禁制) with real collected data');
});

test('wiring: Commit Gate verdict persister writes .omc/logs/{runId}/gate-verdicts.json (§1.1 doc claim made real)', () => {
  assert.ok(src.includes('gate-verdicts.json'), 'Commit Gate verdict log path present');
  assert.match(src, /EVIDENCE PERSISTER \(Issue #152 gate-verdicts\)/,
    'dedicated gate-verdicts persister agent (distinct from the release-review-verdicts.md one)');
  assert.ok(src.includes("label: 'gate:persist-verdicts'"), 'persister runs as its own labeled agent in the Commit Gate phase');
  assert.match(src, /perLane: \(gateReviews \|\| \[\]\)\.map\(r => \(\{ dimension: r && r\.dimension, verdict: r && r\.verdict \}\)\)/,
    'persisted JSON carries the real per-lane dimension/verdict pairs');
});

test('wiring: persistedLogOk derives from parsing the persister report (fail-closed, POSTED-symmetric)', () => {
  assert.match(src, /persistedLogOk = \/persisted\/i\.test\(String\(persisterResult \|\| ''\)\)/,
    'persistedLogOk parsed from the persister report token');
  assert.ok(!/\bpersistedLogOk = true\b/.test(src), 'no unconditional persistedLogOk = true remains');
  assert.match(src, /報告に "persisted" を含めること/, 'persister prompt requires the persisted token in its report');
});

test('wiring: PR_NUMBER missing → posting skipped, persistence log only (UC-5 edge)', () => {
  assert.ok(/extractPrNumber\(releaseResult\)/.test(src), 'PR number extracted from the release agent report');
  assert.ok(src.includes('PR_NUMBER not extractable'), 'skip reason made explicit');
  assert.match(src, /persistence log only|永続ログのみ/, 'skip case keeps the persistent log as the evidence floor');
});

test('wiring: both-fail path records 未収集 status (fail-closed) in the workflow return', () => {
  assert.ok(src.includes("'未収集'"), "explicit '未収集' status on double failure");
  const retIdx = src.indexOf('persistedLog:');
  assert.ok(retIdx > 0, 'return value exposes persistedLog');
  assert.ok(/prReviewsPosted/.test(src.slice(retIdx - 200, retIdx + 400)), 'return value exposes prReviewsPosted');
});
