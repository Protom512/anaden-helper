
export const PROTOCOL_SPEC = {
  id: 'team-gate-protocol',
  issue: 63,
  phase: 'design (not wired)',
  verdictTransport:
    'structured-first: SendMessage body carries a fenced JSON envelope (GATE_VERDICT_JSON); R6 lenient keyword text parse is the last-resort fallback only',
  lanes: 6,
  lifecycle: [
    'TeamCreate(team_name: "commit-gate-<run-id>")',
    'spawn 6 gate reviewers as teammates (names: gate-reliability ... gate-integration)',
    'each teammate reviews (R8 diff-inject is embedded in the spawn prompt) and replies via SendMessage(to: "team-lead", message: GATE_VERDICT_JSON envelope)',
    'team-lead aggregates envelopes; null/missing lanes retry once with opus (R9), then R6 text fallback',
    'gate incomplete after fallback → refuse release (same semantics as current pipeline)',
    'CONSENSUS JUDGE runs on the team-lead side with the deterministic aggregate pre-verdict attached',
    'SendMessage(shutdown_request) to all teammates → TeamDelete',
  ],
  r6Fallback: 'lenient verdict keyword parse (NO-GO|NOGO|CONDITIONAL|GO, word-boundary, NO-GO tested first) — identical to the in-pipeline R6 mitigation',
  r9Retry: 'one opus retry per null lane before the text fallback — identical to the in-pipeline R9 mitigation',
  wiringConditions: [
    'Task 1 (standalone issue): TeamCreate/SendMessage invocation from the workflow sandbox must be proven feasible before any wiring.',
    'Quantified benefit: measured Commit-Gate wall-clock with current parallel() must improve by >=30% under TeamCreate, else keep parallel().',
    'Structured verdict transport must be preserved (JSON envelope); plain-text parse only as R6 last-resort fallback.',
    'Semantics invariance: 6/6 dimensions required (gate-incomplete refusal), ANY NO-GO with critical finding → NO-GO, Release Review 3/3 GO unaffected.',
  ],
};

const VERDICT_ENUM = ['GO', 'NO-GO', 'CONDITIONAL'];

const CANONICAL_GATE_LANES = [
  'reliability', 'performance', 'extensibility', 'governance', 'security', 'integration',
];

export function resolveGateLanes({ diffKind, dimensions } = {}) {
  const full = Array.isArray(dimensions) && dimensions.length > 0
    ? dimensions.map(String)
    : CANONICAL_GATE_LANES;
  const keep = (keys) => {
    const lanes = full.filter((k) => keys.includes(k));
    return lanes.length > 0 ? lanes : full;
  };
  if (diffKind === 'docs-only') {
    return { lanes: keep(['governance', 'integration']), shortCircuited: true, reason: 'diff-kind:docs-only' };
  }
  return { lanes: full, shortCircuited: false, reason: `diff-kind:${diffKind === 'code' || diffKind === 'mixed' ? diffKind : 'unknown(full,fail-closed)'}` };
}

function coerceVerdict(raw) {
  if (typeof raw !== 'string') return null;
  const upper = raw.toUpperCase();
  if (upper === 'NOGO') return 'NO-GO';
  return VERDICT_ENUM.includes(upper) ? upper : null;
}

export function parseStructuredEnvelope(text, dimension) {
  if (typeof text !== 'string' || text.length === 0) return null;
  const fenced = text.match(/```(?:json)?\s*(\{[\s\S]*?\})\s*```/);
  const bare = fenced ? null : text.match(/\{[\s\S]*"verdict"[\s\S]*\}/);
  const candidate = fenced ? fenced[1] : bare ? bare[0] : null;
  if (!candidate) return null;
  let obj;
  try {
    obj = JSON.parse(candidate);
  } catch {
    return null;
  }
  const verdict = coerceVerdict(obj.verdict);
  if (!verdict) return null;
  return {
    verdict,
    dimension: typeof obj.dimension === 'string' && obj.dimension ? obj.dimension : dimension,
    findings: Array.isArray(obj.findings)
      ? obj.findings.filter((f) => f && typeof f === 'object' && f.severity && f.title && f.detail)
      : [],
    summary: typeof obj.summary === 'string' ? obj.summary : '',
  };
}

export function parseVerdictText(text) {
  if (typeof text !== 'string') return null;
  const m = text.match(/\b(?:VERDICT:\s*)?(NO-GO|NOGO|CONDITIONAL|GO)\b/i);
  if (!m) return null;
  return coerceVerdict(m[1]);
}

export function aggregateGateVerdicts(reviews, opts = {}) {
  const expected = opts.expectedDimensions ?? 6;
  const list = Array.isArray(reviews) ? reviews : [];
  const present = list.filter(Boolean);
  const missing = Array.from({ length: expected - present.length }, () => '?');
  if (missing.length > 0 || present.length < expected) {
    return { preVerdict: 'INCOMPLETE', blocking: [], missing };
  }
  const hasCriticalNoGo = present.some(
    (r) =>
      r.verdict === 'NO-GO' &&
      (!Array.isArray(r.findings) ||
        r.findings.length === 0 ||
        r.findings.some((f) => (f.severity || '').toLowerCase() === 'critical'))
  );
  if (hasCriticalNoGo) {
    return { preVerdict: 'NO-GO', blocking: present.filter((r) => r.verdict === 'NO-GO').map((r) => r.dimension), missing: [] };
  }
  const anyNoGo = present.some((r) => r.verdict === 'NO-GO');
  if (anyNoGo) {
    return { preVerdict: 'NO-GO', blocking: present.filter((r) => r.verdict === 'NO-GO').map((r) => r.dimension), missing: [] };
  }
  if (present.every((r) => r.verdict === 'GO')) {
    return { preVerdict: 'GO', blocking: [], missing: [] };
  }
  return { preVerdict: 'CONDITIONAL', blocking: [], missing: [] };
}

export function planRetry(dimensions, results, opts = {}) {
  const retried = new Set(opts.retried || []);
  const retry = [];
  const fallback = [];
  for (const d of dimensions) {
    const v = results[d];
    if (v != null) continue;
    if (retried.has(d)) fallback.push(d);
    else retry.push(d);
  }
  return { retry, fallback };
}

export function buildReviewerTeammatePrompt({ key, dimensionPrompt, teamLead = 'team-lead' }) {
  return `${dimensionPrompt}

【teammate プロトコル (Issue #63 Task 2)】あなたは Commit Gate レーン "${key}" の teammate レビュアーです。
レビューを完了したら、必ず SendMessage ツールで team 宛先 "${teamLead}" へ返信すること。
返信メッセージの本文には、以下の fenced JSON envelope を**必ず**含めること
(構造化 verdict が第一経路 — prose-only はレビュー消失として扱われる):

\`\`\`
GATE_VERDICT_JSON
{"verdict":"GO|NO-GO|CONDITIONAL","dimension":"${key}","findings":[{"severity":"...","title":"...","detail":"...","evidence":"...","suggestion":"..."}],"summary":"..."}
\`\`\`

【R6 フォールバック】何らかの理由で JSON envelope を構成できない場合のみ、返信本文の
どこか（最終行を推奣）に verdict キーワード GO / NO-GO / CONDITIONAL を含めること
（VERDICT: プレフィックス任意・大小問わず。team-lead 側で lenient parse される）。`;
}

export function buildConsensusJudgePrompt(reviews, aggregate) {
  return `CONSENSUS JUDGE (team-lead 側で実行 — Issue #63 Task 2).

Reviews (aggregated from SendMessage envelopes):
${JSON.stringify(reviews, null, 2)}

Deterministic pre-verdict computed by the protocol (confirm or overturn with reasons):
${JSON.stringify(aggregate)}

Rules:
- ANY NO-GO with critical finding → NO-GO
- All GO → GO
- CONDITIONAL → GO if conditions are minor
- preVerdict INCOMPLETE は再審査を要求 (partialevidence から GO を合成しない)
- BASELINE DEBT はブロック要因ではない (integration レーンの baseline-debt 起票済み issue は follow_up_items へ)

Provide: verdict matrix / blocking issues / judgment calls / follow-up items /
final verdict (GO or NO-GO) / commit message + body if GO.`;
}

export function decideReviewRoute(globals, opts = {}) {
  const required = ['TeamCreate', 'SendMessage', 'TaskCreate', 'TaskList', 'TaskUpdate'];
  const g = globals && typeof globals === 'object' ? globals : {};
  const allPresent = required.every((k) => typeof g[k] === 'function');
  if (!allPresent) return { route: 'fallback', reason: 'primitives-unavailable' };
  if (opts.teamCreateError) return { route: 'fallback', reason: 'team-create-failed' };
  return { route: 'team', reason: 'primitives-available' };
}
