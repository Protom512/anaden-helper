// Tests for the TeamCreate-based Commit Gate protocol (Issue #63 Task 2 — design).
// TDD: written BEFORE the implementation. Run: node --test .claude/workflows/team-gate-protocol.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';

import {
  PROTOCOL_SPEC,
  parseStructuredEnvelope,
  parseVerdictText,
  aggregateGateVerdicts,
  planRetry,
  buildReviewerTeammatePrompt,
  buildConsensusJudgePrompt,
  decideReviewRoute,
} from './team-gate-protocol.js';

// ── parseStructuredEnvelope: structured verdict transport (preferred path) ──

test('parseStructuredEnvelope: valid JSON envelope yields structured verdict', () => {
  const text = 'review done\n```json\n{"verdict":"GO","dimension":"reliability","findings":[],"summary":"ok"}\n```';
  const v = parseStructuredEnvelope(text, 'reliability');
  assert.deepEqual(v, { verdict: 'GO', dimension: 'reliability', findings: [], summary: 'ok' });
});

test('parseStructuredEnvelope: bare JSON object without fences is accepted', () => {
  const v = parseStructuredEnvelope('{"verdict":"NO-GO","dimension":"security","findings":[{"severity":"critical","title":"t","detail":"d"}],"summary":"s"}', 'security');
  assert.equal(v.verdict, 'NO-GO');
  assert.equal(v.findings.length, 1);
});

test('parseStructuredEnvelope: returns null on non-JSON prose', () => {
  assert.equal(parseStructuredEnvelope('I think overall it is fine', 'reliability'), null);
});

test('parseStructuredEnvelope: rejects JSON with invalid verdict enum', () => {
  assert.equal(parseStructuredEnvelope('{"verdict":"MAYBE","dimension":"x","findings":[],"summary":"s"}', 'x'), null);
});

// ── parseVerdictText: R6 lenient keyword fall-back (maintained on teammate protocol) ──

test('parseVerdictText: VERDICT: GO prefix', () => {
  assert.equal(parseVerdictText('VERDICT: GO'), 'GO');
});

test('parseVerdictText: NO-GO tested before GO (no false GO)', () => {
  assert.equal(parseVerdictText('verdict: NO-GO'), 'NO-GO');
  assert.equal(parseVerdictText('全体として NO-GO'), 'NO-GO');
});

test('parseVerdictText: NOGO normalized to NO-GO', () => {
  assert.equal(parseVerdictText('Nogo accepted? no — final: NOGO'), 'NO-GO');
});

test('parseVerdictText: keyword anywhere, case-insensitive', () => {
  assert.equal(parseVerdictText('conditional: 要確認'), 'CONDITIONAL');
  assert.equal(parseVerdictText('no-go'), 'NO-GO');
});

test('parseVerdictText: does not match GOOD/GOING', () => {
  assert.equal(parseVerdictText('The code is going in a good direction'), null);
});

test('parseVerdictText: null on no keyword', () => {
  assert.equal(parseVerdictText('特筆すべき問題なし'), null);
});

// ── aggregateGateVerdicts: CONSENSUS JUDGE rules (team-lead side, deterministic pre-check) ──

const rv = (verdict, dimension, extra = {}) => ({ verdict, dimension, findings: [], summary: '', ...extra });

test('aggregate: all GO → GO', () => {
  const dims = ['reliability','performance','extensibility','governance','security','integration'];
  const r = aggregateGateVerdicts(dims.map((d) => rv('GO', d)));
  assert.equal(r.preVerdict, 'GO');
  assert.equal(r.blocking.length, 0);
});

test('aggregate: any NO-GO → NO-GO', () => {
  const dims = ['reliability','performance','extensibility','governance','security','integration'];
  const reviews = dims.map((d) => rv('GO', d));
  reviews[4] = rv('NO-GO', 'security', { findings: [{ severity: 'critical', title: 't', detail: 'd' }] });
  const r = aggregateGateVerdicts(reviews);
  assert.equal(r.preVerdict, 'NO-GO');
});

test('aggregate: missing dimension → incomplete (never synthesize from partial evidence)', () => {
  const reviews = ['reliability','performance'].map((d) => rv('GO', d)); // only 2 of 6
  const r = aggregateGateVerdicts(reviews, { expectedDimensions: 6 });
  assert.equal(r.preVerdict, 'INCOMPLETE');
  assert.ok(r.missing.length > 0);
});

test('aggregate: null entries count as missing, not as GO', () => {
  const reviews = ['reliability','performance','extensibility','governance','security'].map((d) => rv('GO', d));
  reviews.push(null);
  const r = aggregateGateVerdicts(reviews, { expectedDimensions: 6 });
  assert.equal(r.preVerdict, 'INCOMPLETE');
  assert.deepEqual(r.missing, ['?']); // dimension unknown for null lane
});

// ── planRetry: R9 retry maintained on the teammate protocol ──

test('planRetry: healthy first pass → no retry, no fallback', () => {
  const dims = ['reliability','performance'];
  const results = { reliability: rv('GO', 'reliability'), performance: rv('GO', 'performance') };
  const p = planRetry(dims, results);
  assert.equal(p.retry.length, 0);
  assert.equal(p.fallback.length, 0);
});

test('planRetry: null lane → retry once (R9, opus)', () => {
  const dims = ['reliability', 'performance'];
  const results = { reliability: null, performance: rv('GO', 'performance') };
  const p = planRetry(dims, results);
  assert.deepEqual(p.retry, ['reliability']);
  assert.equal(p.fallback.length, 0);
});

test('planRetry: null after retry → R6 text fallback lane', () => {
  const dims = ['security'];
  const afterRetry = { security: null };
  const p = planRetry(dims, afterRetry, { retried: ['security'] });
  assert.equal(p.retry.length, 0);
  assert.deepEqual(p.fallback, ['security']);
});

// ── teammate prompt builders: protocol invariants ──

test('buildReviewerTeammatePrompt: contains structured envelope contract + R6 fallback + reply-to rule', () => {
  const p = buildReviewerTeammatePrompt({ key: 'security', dimensionPrompt: 'SECURITY REVIEWER...', teamLead: 'team-lead' });
  assert.match(p, /SendMessage/);
  assert.match(p, /team-lead/);
  assert.match(p, /GATE_VERDICT_JSON/); // structured envelope must remain (CTO condition: no plain-text-only regression)
  assert.match(p, /NO-GO|NOGO|CONDITIONAL|GO/); // R6 keyword fallback maintained
});

test('buildConsensusJudgePrompt: deterministic pre-verdict + judge role on team-lead side', () => {
  const reviews = [rv('GO', 'a'), rv('GO', 'b')];
  const p = buildConsensusJudgePrompt(reviews, { preVerdict: 'GO', blocking: [], missing: [] });
  assert.match(p, /CONSENSUS JUDGE/);
  assert.match(p, /"preVerdict":"GO"/);
});

// ── decideReviewRoute: team vs fall-back routing after primitive check (Issue #63 Task 6) ──

test('decideReviewRoute: full primitives → team route', () => {
  const r = decideReviewRoute({
    TeamCreate: () => {}, SendMessage: () => {}, TaskCreate: () => {}, TaskList: () => {}, TaskUpdate: () => {},
  });
  assert.equal(r.route, 'team');
  assert.equal(r.reason, 'primitives-available');
});

test('decideReviewRoute: any missing primitive → fallback route', () => {
  const full = { TeamCreate: () => {}, SendMessage: () => {}, TaskCreate: () => {}, TaskList: () => {}, TaskUpdate: () => {} };
  for (const k of Object.keys(full)) {
    const partial = { ...full };
    delete partial[k];
    const r = decideReviewRoute(partial);
    assert.equal(r.route, 'fallback', `${k} missing → fallback`);
    assert.equal(r.reason, 'primitives-unavailable');
  }
});

test('decideReviewRoute: all primitives absent → fallback route', () => {
  const r = decideReviewRoute({});
  assert.equal(r.route, 'fallback');
});

test('decideReviewRoute: TeamCreate failure → fallback route (probe failure degrades, never throws)', () => {
  const r = decideReviewRoute(
    { TeamCreate: () => {}, SendMessage: () => {}, TaskCreate: () => {}, TaskList: () => {}, TaskUpdate: () => {} },
    { teamCreateError: new Error('boom') }
  );
  assert.equal(r.route, 'fallback');
  assert.equal(r.reason, 'team-create-failed');
});

test('decideReviewRoute: throws nothing even with garbage input (null/undefined globals)', () => {
  const r = decideReviewRoute(null);
  assert.equal(r.route, 'fallback');
});

test('PROTOCOL_SPEC: documents CTO DEFER wiring conditions', () => {
  assert.ok(Array.isArray(PROTOCOL_SPEC.wiringConditions));
  assert.ok(PROTOCOL_SPEC.wiringConditions.some((c) => /TeamCreate.*(サンドボックス|sandbox|呼び出し|invocable)/i.test(c) || /Task 1/i.test(c)));
  assert.ok(PROTOCOL_SPEC.wiringConditions.some((c) => /30%|所要時間|wall.?clock|定量/i.test(c)));
  assert.match(PROTOCOL_SPEC.verdictTransport, /structured/i);
});
