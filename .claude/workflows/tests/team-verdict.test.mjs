// TDD tests for TeamCreate-based Release Review verdict merge semantics (Task 3, Issue #63).
// The workflow script is not an importable module (harness-executed). We extract the
// marker-delimited pure-logic block from feature-pipeline.js and eval it, so the tests
// exercise the exact code that ships in the workflow.
//
// Run: node .claude/workflows/tests/team-verdict.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [team-verdict-begin]';
const END = '// [team-verdict-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);

let failures = 0;
function assert(cond, msg) {
  if (!cond) {
    failures += 1;
    console.error(`FAIL: ${msg}`);
  } else {
    console.log(`ok: ${msg}`);
  }
}

assert(begin >= 0, 'feature-pipeline.js contains team-verdict-begin marker');
assert(end > begin, 'feature-pipeline.js contains team-verdict-end marker after begin');

if (begin >= 0 && end > begin) {
  const block = src.slice(begin + BEGIN.length, end);
  const api = new Function(`${block}; return { parseTeamVerdict, mergeReleaseVerdicts };`)();

  // ── parseTeamVerdict: strict structured token, NO-GO wins over GO ──
  assert(api.parseTeamVerdict('VERDICT: GO')?.verdict === 'GO', 'parseTeamVerdict accepts VERDICT: GO');
  assert(api.parseTeamVerdict('VERDICT=NO-GO')?.verdict === 'NO-GO', 'parseTeamVerdict accepts VERDICT=NO-GO');
  assert(api.parseTeamVerdict('verdict: go')?.verdict === 'GO', 'parseTeamVerdict is case-insensitive');
  assert(api.parseTeamVerdict('VERDICT:NO-GO (scope regression)')?.verdict === 'NO-GO', 'NO-GO matched before GO (no false GO)');
  assert(api.parseTeamVerdict('VERDICT: GO but earlier NO-GO mention') === null, 'ambiguous multi-verdict message rejected (strict, single structured token)');
  assert(api.parseTeamVerdict('all good, GO ahead') === null, 'prose-only GO rejected — plain text is not a verdict channel');
  assert(api.parseTeamVerdict('') === null, 'empty message → null');
  assert(api.parseTeamVerdict(null) === null, 'null message → null');
  assert(api.parseTeamVerdict('VERDICT: GOING') === null, 'GOING does not match GO (word boundary)');
  assert(api.parseTeamVerdict('VERDICT: CONDITIONAL') === null, 'CONDITIONAL is not a valid release verdict → null (blocks, same as missing)');

  // ── mergeReleaseVerdicts: 3/3 GO semantics unchanged, missing ≠ GO ──
  const GO = { verdict: 'GO' };
  const NOGO = { verdict: 'NO-GO' };
  assert(api.mergeReleaseVerdicts([GO, GO, GO]).allGo === true, '3/3 GO → allGo true');
  assert(api.mergeReleaseVerdicts([GO, GO, NOGO]).allGo === false, '2/3 GO + NO-GO → allGo false');
  assert(api.mergeReleaseVerdicts([GO, GO, null]).allGo === false, '2/3 GO + missing → allGo false (missing blocks, current semantics)');
  assert(api.mergeReleaseVerdicts([null, null, null]).allGo === false, 'all missing → allGo false');
  assert(api.mergeReleaseVerdicts([]).allGo === false, 'empty verdicts → allGo false');
  assert(api.mergeReleaseVerdicts([GO, GO, GO]).goCount === 3, 'goCount counts GO verdicts');
  assert(api.mergeReleaseVerdicts([GO, NOGO, null]).goCount === 1, 'goCount ignores NO-GO and missing');
  assert(api.mergeReleaseVerdicts([GO, GO, GO]).expectedTotal === 3, 'expectedTotal is 3');
  assert(api.mergeReleaseVerdicts([GO, GO, null]).missingCount === 1, 'missingCount counts nulls');
  // malformed entries must not crash or count as GO
  assert(api.mergeReleaseVerdicts([{}, 'GO', undefined]).allGo === false, 'malformed entries never yield allGo');

  // ── release-block behavior: incomplete team review must block merge ──
  const merged = api.mergeReleaseVerdicts([GO, null, NOGO]);
  assert(merged.releaseBlocked === true, 'missing or NO-GO → releaseBlocked true');
  assert(api.mergeReleaseVerdicts([GO, GO, GO]).releaseBlocked === false, '3/3 GO → releaseBlocked false');
}

// ── team vs fall-back route branching (Issue #63 Task 6) ──
// The routing decision (runReleaseReviewViaTeam) is a closure over harness globals,
// so we verify its contract structurally: (a) the pure route predicate lives in
// team-gate-protocol.js and is unit-tested there; (b) the workflow source keeps the
// gate order: primitive check → team attempt → null fall-through to parallel() path;
// (c) both paths converge on mergeReleaseVerdicts (identical semantics).
const teamFnStart = src.indexOf('const runReleaseReviewViaTeam');
const teamFnEnd = src.indexOf('const mergeReleaseVerdicts', teamFnStart);
assert(teamFnStart > 0, 'feature-pipeline.js defines runReleaseReviewViaTeam');
const teamFn = src.slice(teamFnStart, teamFnEnd > 0 ? teamFnEnd : undefined);
assert(/typeof TeamCreate === 'function'/.test(teamFn), 'team path gates on TeamCreate availability');
assert(/typeof SendMessage === 'function'/.test(teamFn), 'team path gates on SendMessage availability');
assert(/return null;/.test(teamFn), 'unavailable primitives → return null (fall-back signal)');
const fbIdx = src.indexOf('falling back to parallel() path');
const gateIdx = src.indexOf("typeof TeamCreate === 'function'");
assert(gateIdx > 0 && fbIdx > gateIdx, 'fall-back log follows the primitive gate');
const useIdx = src.indexOf('await runReleaseReviewViaTeam()');
const branchIdx = src.indexOf('if (releaseReviewMerged)');
assert(useIdx > 0 && branchIdx > useIdx, 'team result is branched on after the attempt');
assert(/const releaseDecision = mergeReleaseVerdicts\(/.test(src.slice(branchIdx)), 'post-branch decision uses mergeReleaseVerdicts (unified semantics)');
// ── Issue #152 Task 2: 実 per-lane verdict (fabrication 廃止) ──
// runReleaseReviewViaTeam must return { collected, merged } — real per-lane
// {lane, verdict, rationale} data. The goCount-derived verdict reconstruction
// (new Array(merged.goCount).fill / i < goCount ternary) is removed; the
// 3/3-GO mergeReleaseVerdicts invariant itself is unchanged (pins above).
assert(src.includes('return { collected, merged };'), 'runReleaseReviewViaTeam returns { collected, merged } (real per-lane data)');
assert(!src.includes('new Array(releaseReviewMerged.goCount)'), 'team-path verdicts are no longer fabricated from goCount');
assert(!src.includes('i < releaseReviewMerged.goCount'), 'releaseDecision input is no longer fabricated from goCount');

if (failures > 0) {
  console.error(`\n${failures} test(s) failed`);
  process.exit(1);
}
console.log('\nAll team-verdict tests passed');

// ── Task 5 regression (2026-08-21): TeamDelete must be called with team_name ──
// Prior defect: finally-block called TeamDelete() with no args → team dir orphaned
// (observed live: "No team name found" / ~/.claude/teams/task5-validation leftover).
{
  const fin = src.match(/finally\s*\{[\s\S]*?TeamDelete\s*\(([^)]*)\)/);
  assert(fin !== null, 'feature-pipeline.js has a finally-block TeamDelete cleanup');
  if (fin) {
    assert(/\ teamName/.test(fin[1]) || fin[1].includes('teamName'), `TeamDelete receives team_name (got args: "${fin[1].trim()}")`);
  }
}
