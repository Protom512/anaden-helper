// ─────────────────────────────────────────────────────────────────────────────
// TeamCreate-based Commit Gate protocol (Issue #63 Task 2 — DESIGN)
// ─────────────────────────────────────────────────────────────────────────────
// Commit Gate 6レーンを TeamCreate teammate として spawn し、verdict を
// SendMessage で集約し、CONSENSUS JUDGE を team-lead 側で実行するプロトコル。
//
// 【状態: 設計のみ・未接続】CTO Estimate DEFER (2026-08-20) により、以下の
// wiring conditions が全て満たされるまで feature-pipeline.js の現行
// `parallel()` + StructuredOutput 経路は変更しない:
//
// wiringConditions:
//   1. Task 1 (独立 issue): workflow スクリプトサンドボックスから TeamCreate /
//      SendMessage が実際に呼び出せることが実証されていること。
//   2. 定量メリット: 現行 parallel() 実装の Commit Gate 所要時間を実測し、
//      TeamCreate 化で 30% 以上のウォールクロック短縮が立証されること。
//   3. verdict 伝達は構造化 (JSON envelope) を維持すること。プレーンテキスト
//      parse への後退は 3/3 GO 判定の信頼性を損なうため不可 — 本プロトコルの
//      第1経路は JSON envelope、テキスト parse (R6) は最終フォールバックのみ。
//
// verdictTransport: structured-first (JSON envelope), R6 keyword text parse as
// last-resort fallback only.
// ─────────────────────────────────────────────────────────────────────────────

export const PROTOCOL_SPEC = {
  id: 'team-gate-protocol',
  issue: 63,
  phase: 'design (not wired)',
  verdictTransport:
    'structured-first: SendMessage body carries a fenced JSON envelope (GATE_VERDICT_JSON); R6 lenient keyword text parse is the last-resort fallback only',
  lanes: 6, // reliability / performance / extensibility / governance / security / integration
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

// ── diff-kind lane short-circuit (Issue #95 / P-008 T3) ──

const CANONICAL_GATE_LANES = [
  'reliability', 'performance', 'extensibility', 'governance', 'security', 'integration',
];

/**
 * Resolve the active gate lane set from the diff kind classification
 * (gate-diff-kind.js). Pure function — never throws.
 *
 * - 'docs-only': short-circuit to a governance-centered 1-2 lane subset
 *   (governance always kept + one code-adjacent lane). Never zero lanes
 *   (approver condition on T3).
 * - 'code' / 'mixed' / unknown / malformed: full lanes (fail-closed default —
 *   an unclassifiable diff can never skip review lanes).
 *
 * @param {{diffKind?: unknown, dimensions?: unknown}} input
 * @returns {{lanes: string[], shortCircuited: boolean, reason: string}}
 */
export function resolveGateLanes({ diffKind, dimensions } = {}) {
  const full = Array.isArray(dimensions) && dimensions.length > 0
    ? dimensions.map(String)
    : CANONICAL_GATE_LANES;
  const keep = (keys) => {
    const lanes = full.filter((k) => keys.includes(k));
    // fail-closed: if the configured subset matches nothing (unknown dimension
    // set), keep the full set rather than collapsing to zero lanes.
    return lanes.length > 0 ? lanes : full;
  };
  if (diffKind === 'docs-only') {
    return { lanes: keep(['governance', 'integration']), shortCircuited: true, reason: 'diff-kind:docs-only' };
  }
  return { lanes: full, shortCircuited: false, reason: `diff-kind:${diffKind === 'code' || diffKind === 'mixed' ? diffKind : 'unknown(full,fail-closed)'}` };
}

// ── structured envelope parse (preferred transport) ──

function coerceVerdict(raw) {
  if (typeof raw !== 'string') return null;
  const upper = raw.toUpperCase();
  if (upper === 'NOGO') return 'NO-GO';
  return VERDICT_ENUM.includes(upper) ? upper : null;
}

/**
 * Parse a teammate's SendMessage body for the structured verdict envelope.
 * Preferred path: a fenced ```json (or bare) JSON object matching V_SCHEMA
 * {verdict, dimension, findings, summary}. Returns the coerced review object,
 * or null when no valid envelope is present (caller then falls back to R6).
 */
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

// ── R6 lenient text fall-back (word-boundary keyword parse) ──

/**
 * Parse a verdict keyword from free text. NO-GO/NOGO are tested before GO so a
 * "NO-GO" never false-matches GO; \b prevents GOOD/GOING false positives.
 * Returns 'GO' | 'NO-GO' | 'CONDITIONAL' or null.
 */
export function parseVerdictText(text) {
  if (typeof text !== 'string') return null;
  const m = text.match(/\b(?:VERDICT:\s*)?(NO-GO|NOGO|CONDITIONAL|GO)\b/i);
  if (!m) return null;
  return coerceVerdict(m[1]);
}

// ── deterministic aggregation (pre-verdict for the CONSENSUS JUDGE) ──

/**
 * Aggregate 6 gate reviews into a deterministic pre-verdict the team-lead
 * attaches to the CONSENSUS JUDGE prompt. Semantics identical to the current
 * pipeline: ANY NO-GO (with critical finding, when findings are available)
 * → NO-GO; any missing/null lane → INCOMPLETE (never synthesize from partial
 * evidence); all GO → GO; CONDITIONAL left for the judge.
 */
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

// ── R9 retry planning (maintained on the teammate protocol) ──

/**
 * Given per-dimension results (envelope-parsed or null) and the set of lanes
 * already retried (R9, opus), plan the next action per null lane:
 * first null → retry (once); still-null-after-retry → R6 text fallback lane.
 */
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

// ── teammate prompt builders ──

/**
 * Spawn prompt for one gate-reviewer teammate. Invariants:
 * - R8 diff-inject: the dimension prompt already embeds the working-tree diff.
 * - Structured-first transport: the reply MUST carry a GATE_VERDICT_JSON envelope.
 * - R6 maintained: if the envelope cannot be produced, a final verdict keyword
 *   (GO / NO-GO / CONDITIONAL) must appear somewhere in the text reply.
 */
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

/**
 * Prompt for the CONSENSUS JUDGE step, executed on the team-lead side after all
 * envelopes are aggregated. The deterministic aggregate pre-verdict is attached
 * so the judge can confirm or overturn it with explicit reasoning.
 */
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

/**
 * Pure route decision for the team vs fall-back branching (Issue #63 Task 6).
 * Mirrors the availability gate in feature-pipeline.js runReleaseReviewViaTeam:
 * the team lane is used only when every required team primitive is callable;
 * otherwise (or when TeamCreate itself fails) the route degrades to the
 * R6-hardened parallel() fall-back path. Never throws.
 *
 * @param {Record<string, unknown>|null|null} globals - harness-provided primitives
 * @param {{teamCreateError?: unknown}} [opts] - captured TeamCreate failure, if any
 * @returns {{route: 'team'|'fallback', reason: string}}
 */
export function decideReviewRoute(globals, opts = {}) {
  const required = ['TeamCreate', 'SendMessage', 'TaskCreate', 'TaskList', 'TaskUpdate'];
  const g = globals && typeof globals === 'object' ? globals : {};
  const allPresent = required.every((k) => typeof g[k] === 'function');
  if (!allPresent) return { route: 'fallback', reason: 'primitives-unavailable' };
  if (opts.teamCreateError) return { route: 'fallback', reason: 'team-create-failed' };
  return { route: 'team', reason: 'primitives-available' };
}
