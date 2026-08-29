// TDD tests for the Commit Gate TeamCreate wiring (Issue #63 Task 4).
// Written BEFORE the implementation. The workflow script is not an importable
// module (harness-executed), so — like team-verdict.test.mjs — we extract the
// marker-delimited pure-logic block from feature-pipeline.js and eval it.
//
// Run: node --test .claude/workflows/tests/team-gate-wiring.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [gate-team-begin]';
const END = '// [gate-team-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);
assert.ok(begin >= 0, 'feature-pipeline.js contains gate-team-begin marker');
assert.ok(end > begin, 'feature-pipeline.js contains gate-team-end marker after begin');

const block = src.slice(begin + BEGIN.length, end);
const api = new Function(`${block}; return { normalizeGateReview, aggregateGateReviews };`)();

// ── normalizeGateReview: structured-first transport (PROTOCOL_SPEC) ──

test('normalizeGateReview: structured object passes through (first path)', () => {
  const v = api.normalizeGateReview(
    { verdict: 'GO', dimension: 'reliability', findings: [], summary: 'ok' },
    'reliability'
  );
  assert.deepEqual(v, { verdict: 'GO', dimension: 'reliability', findings: [], summary: 'ok' });
});

test('normalizeGateReview: NOGO object coerces to NO-GO', () => {
  const v = api.normalizeGateReview({ verdict: 'NOGO' }, 'security');
  assert.equal(v.verdict, 'NO-GO');
});

test('normalizeGateReview: fenced JSON envelope in a string is parsed (structured 2nd path)', () => {
  const text = 'review done\n```\nGATE_VERDICT_JSON\n{"verdict":"NO-GO","dimension":"security","findings":[{"severity":"critical","title":"t","detail":"d"}],"summary":"s"}\n```';
  const v = api.normalizeGateReview(text, 'security');
  assert.equal(v.verdict, 'NO-GO');
  assert.equal(v.findings.length, 1);
});

test('normalizeGateReview: bare JSON object string is parsed', () => {
  const v = api.normalizeGateReview('{"verdict":"CONDITIONAL","summary":"c"}', 'governance');
  assert.equal(v.verdict, 'CONDITIONAL');
  assert.equal(v.dimension, 'governance'); // dimension backfilled from lane key
});

test('normalizeGateReview: R6 keyword text parse is the LAST resort only', () => {
  const v = api.normalizeGateReview('全体として GO と判定する', 'performance');
  assert.equal(v.verdict, 'GO');
  assert.equal(v.findings.length, 0);
  assert.match(v.summary, /R6/);
});

test('normalizeGateReview: NO-GO text never false-matches GO', () => {
  assert.equal(api.normalizeGateReview('verdict: no-go', 'x').verdict, 'NO-GO');
  assert.equal(api.normalizeGateReview('NOGO', 'x').verdict, 'NO-GO');
});

test('normalizeGateReview: null on non-verdict input (lane stays MISSING → blocks)', () => {
  assert.equal(api.normalizeGateReview(null, 'x'), null);
  assert.equal(api.normalizeGateReview(undefined, 'x'), null);
  assert.equal(api.normalizeGateReview('', 'x'), null);
  assert.equal(api.normalizeGateReview('no verdict here at all', 'x'), null);
  assert.equal(api.normalizeGateReview({ verdict: 'MAYBE' }, 'x'), null);
  assert.equal(api.normalizeGateReview(42, 'x'), null);
});

// ── aggregateGateReviews: deterministic pre-verdict (semantics invariance) ──

test('aggregateGateReviews: 6/6 GO → complete, all GO', () => {
  const a = api.aggregateGateReviews(Array.from({ length: 6 }, (_, i) => ({ verdict: 'GO', dimension: `d${i}` })));
  assert.equal(a.complete, true);
  assert.equal(a.goCount, 6);
  assert.equal(a.noGoCount, 0);
  assert.deepEqual(a.missing, []);
});

test('aggregateGateReviews: any NO-GO lane → NO-GO, blocking lists its dimensions', () => {
  const reviews = Array.from({ length: 6 }, (_, i) => ({ verdict: i === 3 ? 'NO-GO' : 'GO', dimension: `d${i}` }));
  const a = api.aggregateGateReviews(reviews);
  assert.equal(a.preVerdict, 'NO-GO');
  assert.deepEqual(a.blocking, ['d3']);
});

test('aggregateGateReviews: missing lane → INCOMPLETE, never synthesized (gate-incomplete refusal)', () => {
  const reviews = Array.from({ length: 5 }, () => ({ verdict: 'GO', dimension: 'd' }));
  reviews.push(null);
  const a = api.aggregateGateReviews(reviews, 6);
  assert.equal(a.preVerdict, 'INCOMPLETE');
  assert.equal(a.complete, false);
});

test('aggregateGateReviews: mixed GO/CONDITIONAL with no NO-GO → CONDITIONAL', () => {
  const reviews = Array.from({ length: 6 }, (_, i) => ({ verdict: i < 5 ? 'GO' : 'CONDITIONAL', dimension: `d${i}` }));
  assert.equal(api.aggregateGateReviews(reviews).preVerdict, 'CONDITIONAL');
});

// ── wiring structure assertions (feature-pipeline.js source-level invariants) ──

test('wiring: gate team path is capability-gated (falls back to parallel() path)', () => {
  assert.match(src, /runCommitGateViaTeam/, 'runCommitGateViaTeam exists');
  assert.match(src, /team primitives unavailable.*Commit Gate/s, 'unavailability log + fallback present');
  // The team path must return null when primitives are missing so the existing
  // parallel() route runs unchanged.
  assert.match(src, /gateTeamPrimitivesAvailable/, 'primitive availability helper exists');
});

test('wiring: TeamDelete is called with the team name (no-arg bug fixed)', () => {
  assert.match(src, /TeamDelete\(\{\s*team_name:\s*teamName\s*\}\)/, 'TeamDelete({team_name}) in gate path');
  const rel = src.match(/finally\s*\{[\s\S]{0,400}?\}/);
  assert.ok(rel, 'finally block present');
  assert.doesNotMatch(src, /TeamDelete\(\)\s*;/, 'no bare TeamDelete() calls remain');
});

test('wiring: DEFER comment updated to record Task 4 wiring + activation conditions', () => {
  assert.match(src, /Task 4 .*wired/s, 'Task 4 wiring noted');
  assert.match(src, /capability-gated/s, 'capability-gated activation noted');
});
