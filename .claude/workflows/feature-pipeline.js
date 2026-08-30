export const meta = {
  name: 'feature-pipeline',
  description: 'Full feature pipeline: Coordinate → Request → Estimate → Approve → Implement → Commit Gate → Release(commit→push→PR) → Release Review(3レビュアーGO/NO-GO) → Merge&Close',
  phases: [
    { title: 'Coordinate', detail: 'Coordinator scans backlog, builds dependency graph, recommends or validates task' },
    { title: 'Request', detail: 'PM creates ticket from validated request' },
    { title: 'Estimate', detail: 'Tech Lead estimates, CTO approves' },
    { title: 'Implement', detail: 'Engineers implement with TDD' },
    { title: 'Commit Gate', detail: '6-dimension non-functional review + consensus' },
    { title: 'Release', detail: 'ブランチ作成→コミット→push→PR作成(Closes #N)' },
    { title: 'Release Review', detail: '課題解決GO/NO-GOを3レビュアーで並列判定(ticket受け入れ基準・証拠・副作用)' },
    { title: 'Merge & Close', detail: '3/3 GO時のみsquash merge→ブランチ削除→issue close' },
    { title: 'Self-Improve', detail: 'org-feedback全エントリ→構造カテゴリ集計→局所最適化を回避した根源改善案(人間承認)' },
  ],
};

// [run-metadata-begin]
// Issue #97 UC-1: run 開始時刻 (ISO 8601 タイムゾーン付き) と runId を pipeline
// 起動直後 (Phase 0 Coordinate 前) に生成する。runId は filesystem-safe
// (Windows コロ制約) で runTimestamp と分離 — タイムスタンプ文字列は ':' を
// 含むためディレクトリ名には使えない。helpers は ESM import 不可のため inline
// (drift-guard: tests/run-metadata.test.mjs が marker block を抽出して検証)。
// Workflow runtime は Date.now()/new Date() を禁止 (resume 安全性) ため、
// timestamp は args 経由で受け取る (メインループが ISO 8601 を注入)。
// 未指定時は 'unknown' で fail-closed 扱い (.claude/rules/pipeline-evidence-verification.md)。
// NOTE: ランタイムは export 構文を剥がすため meta は未定義 — phases は args 経由で
// 受け取る (未指定時は meta.phases と同じ既定リストを inline)。
const PIPELINE_PHASES = ['Coordinate', 'Request', 'Estimate', 'Approve', 'Implement', 'Commit Gate', 'Release', 'Release Review', 'Merge & Close', 'Self-Improve'];
const argsObj = (typeof args !== 'undefined' && args && typeof args === 'object') ? args : {};
const runTimestamp = argsObj.runTimestamp || 'unknown'; // ISO 8601 with TZ (Z 形式)
const runId = argsObj.runId || 'run-manual';
const PHASES = argsObj.phases || PIPELINE_PHASES;
const ISO_8601_TZ_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;
function isIso8601WithTimezone(value) {
  return typeof value === 'string' && ISO_8601_TZ_RE.test(value);
}
function buildRunMetadata(input) {
  const errors = [];
  if (input == null || typeof input !== 'object') {
    return { value: null, errors: ['input: missing (object required)'] };
  }
  if (input.runTimestamp == null) errors.push('runTimestamp: missing');
  else if (!isIso8601WithTimezone(input.runTimestamp)) errors.push('runTimestamp: not ISO 8601 with timezone');
  if (input.runId == null) errors.push('runId: missing');
  else if (typeof input.runId !== 'string') errors.push('runId: not a string');
  if (input.issue == null) errors.push('issue: missing');
  else if (typeof input.issue !== 'number') errors.push('issue: not a number');
  if (input.title == null) errors.push('title: missing');
  else if (typeof input.title !== 'string') errors.push('title: not a string');
  if (input.phases == null) errors.push('phases: missing');
  else if (!Array.isArray(input.phases) || input.phases.length === 0
    || !input.phases.every((p) => typeof p === 'string')) {
    errors.push('phases: non-empty string array required');
  }
  if (errors.length > 0) return { value: null, errors }; // fail-closed
  return {
    value: {
      runTimestamp: input.runTimestamp,
      runId: input.runId,
      issue: input.issue,
      title: input.title,
      phases: input.phases,
    },
    errors: [],
  };
}
// [run-metadata-end]

// ── Phase 0: Coordinate ──
phase('Coordinate');
const coordinatorArgs = (typeof args === 'string' ? args.trim() : '') || '';
const isBacklogPick = !coordinatorArgs || /^next$/i.test(coordinatorArgs);

const recommendation = await agent(
  `You are the Project Coordinator. You manage the backlog, analyze dependencies, and recommend the best next task.

## Mode: ${isBacklogPick ? 'BACKLOG_PICK' : 'REQUEST_VALIDATION'}

${isBacklogPick
  ? 'The CEO said "next" or gave no instruction. Scan all backlog sources, build a dependency graph, score tasks, and recommend the top item.'
  : `The CEO requested: "${coordinatorArgs}". Validate this against the backlog. Push back if doing something else first would be better.`}

## Step 1: Scan Backlog Sources

Run these commands to gather backlog data:

1. GitHub Issues (open):
   Run: gh issue list --repo protom512/anaden-helper --state open --json number,title,labels
   If gh is not available, skip this source.

2. Spec tasks (incomplete):
   Read files matching .kiro/specs/*/tasks.md
   Count incomplete tasks (lines starting with "- [ ]").

3. Remaining items:
   Read the project memory at C:/Users/black/.claude/projects/C--Users-black-git-repo-anaden-helper/memory/MEMORY.md
   Look for "Remaining Items" section.
   Also read .claude/handover.md for "残タスク" section.

## Step 2: Build Dependency Graph

anaden-helper はゲーム自動化ツール（Another Eden）。ワークスペース crate 階層:
- anaden-core（ドメイン型）<- anaden-device（capture/input）+ anaden-vision（template matching）<- anaden-engine（pipeline driver）<- anaden-strategies / anaden-cli / anaden-studio
既知のブロッカー:
- Issue #12（PC タイトル cold-start）: 実機 title_pc_probe.png 取得が HARD-blocker（operator-gated）
- PC(16:9, RAW 1258x708) と Android(20:9) のアスペクト比分割: テンプレは比率越境で再利用不可

## Step 3: Score and Rank

Priority formula: score = impact_score * 2 + priority_weight + readiness_bonus
- priority_weight: P1=4, P2=3, P3=2, no-label=1
- readiness_bonus: +3 if spec has approved tasks.md ready for implementation

## Step 4: Present Recommendation

Present to the CEO:
1. Top 3 candidates ranked by score
2. For each: title, source, dependency chain, impact analysis, effort estimate
3. Your recommended pick with clear reasoning

## Step 4b: Fall-through rule (CRITICAL — backlog_pick mode)

In backlog_pick mode, operator-gated / HARD-blocked items (e.g. real-device captures,
physical-game-operation, "human-in-the-loop") CANNOT be dispatched by an executor.
A blocked top item MUST NOT cause a batch veto.

- Rank ALL candidates by score.
- Identify which are EXECUTABLE now (no unsatisfied dependency, NOT operator-gated, NOT HARD-blocked).
- Your "recommendation" MUST be the HIGHEST-SCORED EXECUTABLE candidate — proceed with it
  and set veto=false. Note any higher-scored blocked items as "deferred (blocked)" in your
  reasoning, but do NOT veto on their account.
- Set veto=true ONLY on TRUE backlog exhaustion: NO executable candidate exists at all.
  In that case still list the blocked candidates and what would unblock them.
- Never conflate "the top item is blocked" with "nothing can be done".

${!isBacklogPick ? `## Step 5: Validate CEO Request

The CEO wants: "${coordinatorArgs}"

Check:
1. Does this overlap with existing backlog items?
2. Is there a prerequisite that should be done first?
3. Would doing X first make Y (the CEO's request) easier or faster?
4. Is the request scope-appropriate or should it be split?

CONDITIONAL VETO: If the CEO's request has clearly reversed dependencies
(e.g., authoring PC scene templates before the Win32/scrcpy capture backend exists, or a strategy before the pipeline driver),
you MUST set veto=true and explain why.

If push-back is warranted, explain clearly with evidence.
If the request is good, confirm and note related backlog items.` : ''}

Even when pushing back, your "recommendation" field MUST contain a description of what to proceed with.`,
  { label: 'coordinator:analyze', phase: 'Coordinate', model: 'opus', schema: {
    type: 'object',
    properties: {
      mode: { type: 'string', enum: ['backlog_pick', 'request_validation'] },
      validated: { type: 'boolean' },
      recommendation: { type: 'string' },
      candidates: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            title: { type: 'string' },
            source: { type: 'string' },
            score: { type: 'number' },
            impact_score: { type: 'number' },
            effort: { type: 'string', enum: ['S', 'M', 'L', 'XL'] },
            depends_on: { type: 'array', items: { type: 'string' } },
            blocks: { type: 'array', items: { type: 'string' } },
          },
          required: ['title', 'source', 'score'],
        },
      },
      veto: { type: 'boolean' },
      veto_reason: { type: 'string' },
      push_back: { type: 'string' },
      dependency_chain: { type: 'string' },
    },
    required: ['mode', 'recommendation', 'veto'],
  }}
);

log(`Coordinator: ${recommendation.recommendation}`);

// Veto handling: block if vetoed unless CEO explicitly overrides
if (recommendation.veto && !/強行|force|override/i.test(coordinatorArgs)) {
  log(`VETOED: ${recommendation.veto_reason}`);
  return { status: 'vetoed', reason: recommendation.veto_reason, recommendation };
}

const effectiveArgs = isBacklogPick ? recommendation.recommendation : coordinatorArgs;

// ── resolveScope: git を唯一の真実源(SoT)としてスコープを取得 (R2改善) ──
// estimate.tasks[].files 静的リストでなく、git diff HEAD + git status の実行結果を
// 全フェーズ(PM/Estimate/Gate/Release)で共有。これが changed files の唯一の真実源。
const scopeResult = await agent(
  `SCOPE RESOLVER。以下を実行し結果を返す:
1. \`git diff HEAD --name-only\` — HEAD に対する変更ファイル(tracked)
2. \`git status --porcelain\` — 全ファイル状態(tracked+untracked)
両方の出力を結合し、重複排除した changed files リストを返す。
**最後に StructuredOutput({changedFiles: [...], untracked: [...]}) を呼ぶこと。**`,
  { schema: { type: 'object', required: ['changedFiles'], properties: { changedFiles: { type: 'array', items: { type: 'string' } }, untracked: { type: 'array', items: { type: 'string' } } } }, label: 'resolve-scope', phase: 'Coordinate' }
);
const changedFilesList = (scopeResult && scopeResult.changedFiles) ? scopeResult.changedFiles.join(', ') : '(git diff で確認)';
log(`Scope resolved (git SoT): ${changedFilesList.slice(0, 120)}`);

// R5 → Issue #99: 旧 changedCrates 手動導出 (resolve-scope 時点の静的導出) は
// ticket precheck 生成メタデータ (deriveSliceMetadata 実測値) に置換済み —
// ticket-precheck wiring ブロック (Request 直後) で changedCrates / diffKind を
// 上書き宣言 (このコメントに wiring marker 文字列を含めないこと —
// tests/ticket-precheck-wiring.test.mjs の位置検証が誤反応する)。
// ここでは changedFilesArr (contrast / basisFiles 用) のみ保持。
const changedFilesArr = (scopeResult && scopeResult.changedFiles) ? scopeResult.changedFiles : [];
// run-metadata-lookup-begin (drift-guard marker: tests/run-metadata.test.mjs slices from AFTER this comment line to BEFORE the end marker and eval via new Function)
// Issue #97 UC-2/UC-3: 過去実行の照合ヘルパー。.omc/logs/{run-id}/run-metadata.json
// の runTimestamp (run 開始時刻, ISO 8601 tz付き) で過去実行を照合する。
// 注意: runTimestamp は「run 開始時刻」であり、各 evidence ファイルに記録される
// recordedAt / collectedAt (採取時刻) とは別物 — 二重管理ではなく役割分離
// (traceability: どの実行で採取された evidence か)。
// fail-closed: メタデータ欠損 (run-metadata.json 無し / JSON 破損 /
// runTimestamp フィールド無し / 非文字列) ディレクトリは 'unknown' に分類
// (pipeline-evidence-verification.md「空・unknown は green 払いしない」原則)。
// 既存ログ (issue95-p008-verification 等メタデータ無し) は unknown 扱い —
// 照合可能なのは新規 run のみ。
const RUN_TIMESTAMP_UNKNOWN = 'unknown';
function buildRunTimestampIndex(entries) {
  // entries: [{ runId, metadata }] — metadata は run-metadata.json の parse 済み
  // オブジェクト (欠損時 null)。戻り値: Map<runId, { runId, runTimestamp }>
  const index = new Map();
  for (const entry of Array.isArray(entries) ? entries : []) {
    if (!entry || typeof entry.runId !== 'string' || entry.runId === '') continue;
    const ts = entry.metadata && typeof entry.metadata.runTimestamp === 'string'
      ? entry.metadata.runTimestamp.trim()
      : '';
    index.set(entry.runId, {
      runId: entry.runId,
      runTimestamp: ts !== '' ? ts : RUN_TIMESTAMP_UNKNOWN,
    });
  }
  return index;
}
function matchRunByTimestamp(index, runTimestamp) {
  if (!(index instanceof Map) || typeof runTimestamp !== 'string') return null;
  const needle = runTimestamp.trim();
  if (needle === '' || needle === RUN_TIMESTAMP_UNKNOWN) return null;
  for (const entry of index.values()) {
    if (entry.runTimestamp === needle) return entry.runId;
  }
  return null;
}
// run-metadata-lookup-end
// gate-diff-kind-begin (drift-guard marker: tests slice from AFTER this comment line to BEFORE the matching end marker line and eval via new Function)
// Issue #95 (P-008): diff-kind classifier (docs-only / code / mixed) による lane
// short-circuit。gate-diff-kind.js の canonical copy を inline (Workflow runtime は
// ESM import を reject するため script は self-contained 必須 — review-gate.js と
// 同じ構造、drift は tests/gate-diff-kind-wiring.test.mjs が正準比較で guard)。
// Fail-closed (UC-3): 空・malformed・unknown 入力は 'code' に分類され、review lane を
// skip できない構造。docs-only は全 changed path が docs pattern に一致する場合のみ。
const DOCS_DIR_PREFIXES = ['docs/', 'doc/'];
const DOCS_ROOT_PREFIXES = ['.claude/rules/', 'CLAUDE.d/'];
function isDocsPath(path) {
  if (typeof path !== 'string') {
    return false;
  }
  const p = path.replace(/\\/g, '/').trim();
  if (p.length === 0) {
    return false;
  }
  if (p.toLowerCase().endsWith('.md') || p.toLowerCase().endsWith('.markdown')) {
    return true;
  }
  if (DOCS_DIR_PREFIXES.some((prefix) => p.startsWith(prefix))) {
    return true;
  }
  if (DOCS_ROOT_PREFIXES.some((prefix) => p.startsWith(prefix))) {
    return true;
  }
  return false;
}
function classifyDiffKind(changedFiles) {
  if (!Array.isArray(changedFiles) || changedFiles.length === 0) {
    return 'code';
  }
  let sawDocs = false;
  let sawCode = false;
  for (const f of changedFiles) {
    if (typeof f !== 'string' || f.trim().length === 0) {
      // Malformed entry is "unclassified" → fail-closed straight to 'code'
      // (UC-3: structurally prevent a false short-circuit).
      return 'code';
    }
    if (isDocsPath(f)) {
      sawDocs = true;
    } else {
      sawCode = true;
    }
  }
  if (!sawCode) {
    return 'docs-only';
  }
  return sawDocs ? 'mixed' : 'code';
}
// gate-diff-kind-end (drift-guard marker — see begin marker above)
// Issue #99: 旧 Coordinate 時点の diffKind 分類は ticket precheck 生成メタデータ
// で上書きされる (Request 直後の precheck wiring ブロックで再宣言)。ここでは宣言しない。
// SR-3 → P-008 (diff-grounded gate routing): docs-only fast-path。非機能 lane
// (reliability/performance/extensibility) は pure-docs 変更に構造的 N/A。lane 選択は
// GATE_DIMENSIONS 定義側の `lanes: 'code'` メタで行う (governance + integration は
// 常時維持 — fail-closed: 决して 0 lane にはならない。code/mixed は全 6 lane)。
// Release Review (3 reviewers) は引き続き第二 gate として実行される。
// 注意: GATE_DIMENSIONS は ticket を template に使うため後方 (Commit Gate 直前) で
// 宣言される。ACTIVE_GATE_DIMENSIONS への反映もその位置で行う (ここでは分類のみ)。
const DOCS_ONLY_SKIP_KEYS = ['reliability', 'performance', 'extensibility'];
function GATE_DIFF_KIND_LANES() {
  // Issue #99: lane 選択は precheck 生成メタデータ (precheckDiffKind = 実 diff 由来
  // classifyDiffKind 結果) で上書き。diffKind は precheckDiffKind のエイリアス。
  const lanes = GATE_DIMENSIONS.filter(
    (d) => !(precheckDiffKind === 'docs-only' && DOCS_ONLY_SKIP_KEYS.includes(d.key))
  );
  return lanes.length > 0 ? lanes : GATE_DIMENSIONS;
}
// integration gate Step1 は「スライスが crate を触ったか」で分岐(触ってない=docs/data/assets/test のみなら per-slice コード層は N/A)。
// NOTE: hasChangedCrate/changedCratesList は precheck (Issue #99) の後方で宣言されるため、
// 即時評価すると TDZ 違反。関数化して消費時 (integration lane プロンプト構築時) に評価する。
function integrationSliceCheckText() {
  return hasChangedCrate
    ? `各 changed crate (${changedCratesList}) について実行し、緑であることを確認:
       - cargo clippy -p <crate> --all-targets -- -D warnings
       - cargo nextest run -p <crate>
     スライスが導入した失敗は slice-owned → NO-GO(このスライスの責務)。`
    : `スライスは crate を触っていない(docs/data/assets のみ、または test-only)。per-slice コード層は N/A → GO(public API 影響は Step3 で確認)。`;
}

// ── Phase 1: Request ──
phase('Request');
// [pm-ticket-files-begin] (Issue #99 Task 1 / UC-3: drift-guard marker — tests/pm-ticket-files.test.mjs)
// ticket-declared files: PM はチケットが触る予定ファイルを files: string[] で宣言する。
// この宣言が precheck (ticket-precheck) で実 diff と機械検証される (evidence は自己申告不可)。
// 継続系 ("next") 指示時は新機能テンプレート (persona/motivation/use cases) を機械適用せず、
// 未マージブランチ・open PR の残作業サマリ形式 (対象ブランチ/PR/未了工程 + 受け入れ基準) で
// チケット生成する。
const ticket = await agent(
  `You are the Product Manager. Create a structured feature ticket based on this request:

"${effectiveArgs}"

${isBacklogPick
  ? `## Mode: CONTINUATION (継続系指示 — "next" backlog pick)

この指示は継続系 (continuation) である。新機能テンプレートを機械適用してはならない。
まず未マージブランチと open PR を調査せよ:
1. Run: gh pr list --repo protom512/anaden-helper --state open --json number,title,headRefName
2. Run: git branch --no-merged master
3. 最も進行中 (WIP/shard 途中) のブランチ/open PR を特定する。

特定できた場合、チケットは**残作業サマリ形式**で書く:
- 対象ブランチ / PR (番号・タイトル・ブランチ名)
- 未了工程 (残りの実装・テスト・コミット・gate 通過など、完了していない工程のリスト)
- 受け入れ基準 (残作業完了の検証可能な条件)
- ticket-declared files: この残作業が触るファイルを必ず files 配列で宣言する
  (PR-verification や残実装の対象ファイル)。

未マージブランチも open PR も存在しない場合のみ、通常の新機能テンプレートにフォールバックする。`
  : `Use the GitHub Issue template from .claude/agents/org/pm.md.
Include: overview, persona, motivation, use cases (UC-1, UC-2, edge case), acceptance criteria, non-scope.`}

Write the ticket in Japanese.

## ticket-declared files (必須 — Issue #99)

このチケットが触る (変更・新規作成する) 予定のファイルを全て明記し、
StructuredOutput の files フィールドに文字列配列で返すこと。
この宣言は precheck で実際の diff と機械検証される (宣言漏れファイルは fail 対象)。
例: files: ["crates/anaden-cli/src/main.rs", ".claude/workflows/feature-pipeline.js"]

## ticketKind (必須 — Issue #104)

StructuredOutput の ticketKind フィールドに以下のいずれかを返すこと:
- "continuation": 未マージブランチ/open PR の残作業検証・続き (commit-range fallback が適用される)
- "new-implementation": ゼロから新規実装するチケット (実装前に diff は空が正常)

Before creating a new issue, check whether one already exists: run
gh issue list --repo protom512/anaden-helper --state open --search "<keywords>"
and review the results. If an existing open issue already covers this work
(for example, if an open issue already tracks this exact request), do NOT create a
duplicate — put the existing number in issueNumber and reference it.

After creating the ticket content (only if NO duplicate exists), create a
GitHub Issue using: gh issue create`,
  { label: 'pm:create-ticket', phase: 'Request', schema: {
    type: 'object',
    properties: {
      title: { type: 'string' },
      priority: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
      summary: { type: 'string' },
      useCases: { type: 'number' },
      acceptanceCriteria: { type: 'number' },
      issueNumber: { type: 'string' },
      files: { type: 'array', items: { type: 'string' } },
      ticketKind: { type: 'string', enum: ['new-implementation', 'continuation'] },
    },
    required: ['title', 'priority', 'summary', 'files', 'ticketKind'],
  }}
);
log(`Ticket created: ${ticket.title} (Priority: ${ticket.priority})`);
// [pm-ticket-files-end]

// [run-metadata-persist-begin]
// Issue #97 UC-1: run metadata の永続化。ticket の issue/title が確定した時点
// (Request phase 直後) で .omc/logs/{runId}/run-metadata.json を haiku
// persister agent に書き出させる (diff-kind rationale persister と同一パターン)。
// fail-closed: buildRunMetadata が検証エラーを返す場合は空オブジェクトで
// 書き出さず errors を log に残す (evidence は自己申告不可)。
const runMeta = buildRunMetadata({
  runTimestamp,
  runId,
  issue: Number(ticket.issueNumber) || null,
  title: ticket.title,
  phases: PHASES, // meta 未定義 (export剥がし) のため args/inline の PHASES を使用
});
if (runMeta.value) {
  await agent(
    `EVIDENCE PERSISTER (Issue #97 UC-1)。以下の JSON をファイル .omc/logs/${runId}/run-metadata.json へ書き出せ（ディレクトリが無ければ作成。親ディレクトリは repo root の .omc/logs/）。内容はこの JSON をそのまま pretty-print (2-space indent) したもの:
${JSON.stringify(runMeta.value, null, 2)}
書き出し後、書き込んだファイルのパスのみを返せ。`,
    { label: 'run:persist-run-metadata', phase: 'Request', model: 'sonnet' // P-005: haiku は GLM backend で Unknown Model 400 (sonnet へ) }
  );
  log(`Run metadata (Issue #97 UC-1): persisted to .omc/logs/${runId}/run-metadata.json (issue=${runMeta.value.issue}, phases=${runMeta.value.phases.length})`);
} else {
  log(`Run metadata (Issue #97 UC-1): FAIL-CLOSED, not persisted — ${runMeta.errors.join('; ')}`);
}
// [run-metadata-persist-end]

// [ticket-precheck-wiring-begin]
// Issue #99 Task 3: ticket precheck wiring。Request phase (ticket 確定) 直後・
// Estimate 前に evaluateTicketPrecheck を実行し、ticket-declared files と実 diff
// (working-tree + untracked + commit-range fallback) を機械検証する。
// FAIL 時は status 'precheck-failed' で short-circuit (resolveReleaseAbort /
// evidence-failed と同型の resumable status)。PASS 時は slice メタデータ
// (changedCrates / diffKind) を deriveSliceMetadata の実測値で確定し、以降の
// Estimate / Gate プロンプトへ注入する (自己申告メタデータの廃止)。
// Workflow runtime は ESM import を reject するため ticket-precheck.js の
// canonical copy を inline (drift は tests/ticket-precheck-wiring.test.mjs が guard)。
// diff-kind 分類は inline 済み classifyDiffKind (gate-diff-kind.js) を再利用
// (estimate approval condition: 第二の drift surface を作らない)。
// 補足: helper 名は canonical (normalizePath / toNormalizedSet) と同一にする
// (drift-guard test が関数名一致で inline copy を検証する)。
function normalizePath(path) {
  if (typeof path !== 'string') {
    return null;
  }
  let p = path.replace(/\\/g, '/').trim();
  while (p.startsWith('./')) {
    p = p.slice(2);
  }
  p = p.replace(/\/{2,}/g, '/');
  return p.length > 0 ? p : null;
}
function toNormalizedSet(files) {
  const set = new Set();
  const ordered = [];
  let malformed = false;
  if (!Array.isArray(files)) {
    return { set, ordered, malformed: true };
  }
  for (const f of files) {
    const p = normalizePath(f);
    if (p === null) {
      malformed = true;
      continue;
    }
    if (!set.has(p)) {
      set.add(p);
      ordered.push(p);
    }
  }
  return { set, ordered, malformed };
}
// canonical: ticket-precheck.js evaluateTicketPrecheck (verbatim inline copy)
const evaluateTicketPrecheck = (declaredFiles, changedFiles, mode = 'strict') => {
  const declared = toNormalizedSet(declaredFiles);
  const changed = toNormalizedSet(changedFiles);
  const undeclared = [];
  const missing = [];
  for (const p of changed.ordered) {
    if (!declared.set.has(p)) {
      undeclared.push(p);
    }
  }
  for (const p of declared.ordered) {
    if (!changed.set.has(p)) {
      missing.push(p);
    }
  }
  const preImpl = mode === 'pre-implementation';
  const hasMismatch = preImpl ? undeclared.length > 0 : (undeclared.length > 0 || missing.length > 0);
  const fail =
    changed.malformed ||
    (declared.malformed && changed.ordered.length > 0) ||
    (declared.ordered.length === 0 && changed.ordered.length > 0) ||
    (preImpl && declared.ordered.length === 0) ||
    hasMismatch;
  const parts = [];
  if (undeclared.length > 0) {
    parts.push(`undeclared changed files: ${undeclared.join(', ')}`);
  }
  if (missing.length > 0) {
    parts.push(`declared but unchanged files: ${missing.join(', ')}`);
  }
  if (declared.ordered.length === 0 && changed.ordered.length > 0) {
    parts.push('non-empty diff with empty declaration');
  }
  if (changed.malformed) {
    parts.push('malformed changed-files input (fail-closed)');
  }
  if (declared.malformed && changed.ordered.length > 0) {
    parts.push('malformed declaration (fail-closed)');
  }
  return {
    verdict: fail ? 'FAIL' : 'PASS',
    declared: declared.ordered,
    changed: changed.ordered,
    undeclared,
    missing,
    reason: fail ? `ticket-precheck FAIL (${mode}) — ${parts.join('; ')}` : (preImpl && missing.length > 0 ? `ticket-precheck PASS (pre-implementation) — ${missing.length} declared file(s) pending implementation; no undeclared changed files` : 'ticket-precheck PASS — declared files match changed files'),
  };
};
// canonical: ticket-precheck.js evaluateIssuePremise (verbatim inline copy)。
// Issue #109 Task 1: stale (closed+merged) / duplicate (open PR) dispatch 検出。
// fail-closed: malformed/null 入力も FAIL (検証実施不能は dispatch 拒否)。
const evaluateIssuePremise = (input) => {
  const invalid = { verdict: 'FAIL', stale: false, duplicate: false };
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed input (fail-closed: precheck unverifiable, dispatch rejected)' };
  }
  const { issueState, linkedBranchesContainIssue, openPRs } = input;
  if (typeof issueState !== 'string' || (issueState !== 'open' && issueState !== 'closed')) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed issueState (fail-closed: expected "open"|"closed")' };
  }
  if (typeof linkedBranchesContainIssue !== 'boolean') {
    return { ...invalid, reason: 'issue-premise FAIL — malformed linkedBranchesContainIssue (fail-closed: expected boolean)' };
  }
  if (!Array.isArray(openPRs)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed openPRs (fail-closed: expected array)' };
  }
  const stale = issueState === 'closed' && linkedBranchesContainIssue;
  const duplicate = openPRs.length > 0;
  if (stale) {
    return {
      verdict: 'FAIL',
      stale: true,
      duplicate,
      reason: duplicate
        ? 'issue-premise FAIL — stale: issue is closed and already merged into trunk; duplicate: open PR(s) also exist'
        : 'issue-premise FAIL — stale: issue is closed and already merged into trunk',
    };
  }
  if (duplicate) {
    return {
      verdict: 'FAIL',
      stale: false,
      duplicate: true,
      reason: `issue-premise FAIL — duplicate: ${openPRs.length} open PR(s) already reference this issue`,
    };
  }
  return {
    verdict: 'PASS',
    stale: false,
    duplicate: false,
    reason: `issue-premise PASS — issue is ${issueState}, not merged into trunk, no open duplicate PRs`,
  };
};
// canonical: ticket-precheck.js deriveSliceMetadata (verbatim inline copy)。
// crates/* 導出は旧 changedCrates 手動導出 (R5, L209-336 相当) を置換。
const deriveSliceMetadata = (changedFiles) => {
  const changed = toNormalizedSet(changedFiles);
  const crates = changed.ordered
    .filter((f) => f.startsWith('crates/'))
    .map((f) => f.split('/')[1])
    .filter((c) => typeof c === 'string' && c.length > 0)
    .sort();
  return {
    changedCrates: [...new Set(crates)],
    diffKind: classifyDiffKind(changedFiles),
  };
};
// precheck scope resolver: working-tree + untracked + commit-range fallback。
// UC-3: "next" 継続系指示で ticket が PR-verification files を宣言しても
// working tree が clean (commit 済み slice) の場合、HEAD~1..HEAD / merge-base の
// commit-range diff を fallback として結合する (Issue #91 P-007 と同一パターン)。
const precheckScope = await agent(
  `PRECHECK SCOPE RESOLVER (Issue #99)。以下を実行し結果を返す:
1. \`git diff HEAD --name-only\` — working-tree tracked 変更
2. \`git status --porcelain\` — 全ファイル状態。untracked ("?? ") 行はパス部分のみ抽出して untrackedFiles へ
3. commit-range fallback (working-tree diff が空の場合の救済 — UC-3):
   \`git diff HEAD~1..HEAD --name-only\` (HEAD~1 が解決不能な merge context は
   \`git diff \$(git merge-base origin/master HEAD)..HEAD --name-only\`)
   — 結果を commitRangeFiles へ。
4. treeHash (「green だが実体は空」検出用 — pipeline-evidence-verification.md §2):
   \`git write-tree\` の出力 (SHA-1) を treeHash へ (実行不能な場合は空文字列)。
**最後に StructuredOutput({workingTreeFiles: [...], untrackedFiles: [...], commitRangeFiles: [...], treeHash: "..."}) を呼ぶこと。**`,
  { schema: { type: 'object', required: ['workingTreeFiles'], properties: {
    workingTreeFiles: { type: 'array', items: { type: 'string' } },
    untrackedFiles: { type: 'array', items: { type: 'string' } },
    commitRangeFiles: { type: 'array', items: { type: 'string' } },
    treeHash: { type: 'string' },
  } }, label: 'request:precheck-scope', phase: 'Request', model: 'sonnet' }
);
const precheckWorkingTree = (precheckScope && Array.isArray(precheckScope.workingTreeFiles)) ? precheckScope.workingTreeFiles : [];
const precheckUntracked = (precheckScope && Array.isArray(precheckScope.untrackedFiles)) ? precheckScope.untrackedFiles : [];
const precheckRangeFiles = (precheckScope && Array.isArray(precheckScope.commitRangeFiles)) ? precheckScope.commitRangeFiles : [];
// working-tree + untracked を優先し、両方空の場合のみ commit-range fallback (UC-3)。
// Issue #104 修正: commit-range fallback は continuation (未マージ成果物の残作業検証)
// のみに適用。new-implementation (ゼロから実装) では直前の無関係コミット (直前PR等) を
// 自チケットの diff と誤検出するため fallback しない (空 = 実装前の正常状態)。
const precheckTicketKind = (ticket.ticketKind === 'continuation') ? 'continuation' : 'new-implementation';
const precheckChangedFiles = (precheckWorkingTree.length > 0 || precheckUntracked.length > 0)
  ? [...new Set([...precheckWorkingTree, ...precheckUntracked])]
  : (precheckTicketKind === 'continuation' ? [...new Set(precheckRangeFiles)] : []);
// Issue #102 修正: この位置は実装前 (Request->Estimate 間) のため mode='pre-implementation' —
// declared-but-unchanged は FAIL にしない (実装が宣言に先行するのは通常)。
// undeclared (宣言外の実 diff) と malformed/空宣言のみ FAIL。gate 時は strict。
const ticketPrecheck = evaluateTicketPrecheck(ticket.files, precheckChangedFiles, 'pre-implementation');
const precheckSliceMetadata = deriveSliceMetadata(precheckChangedFiles);
// Issue #99: slice メタデータ (changedCrates / diffKind) は precheck 生成の
// 実測値で上書き — Coordinate 時点の resolve-scope 手動導出 (旧 L209-336) を
// 置換 (自己申告・静的導出の排除)。
const changedCrates = precheckSliceMetadata.changedCrates;
const changedCratesList = changedCrates.join(', ');
const hasChangedCrate = changedCrates.length > 0;
// precheck-derived diffKind で GATE_DIMENSIONS lane 選択を上書き。
const precheckDiffKind = precheckSliceMetadata.diffKind;
const diffKind = precheckDiffKind;
// evidence persistence: precheck verdict を .omc/logs/{runId}/ticket-precheck.json
// へ永続化 (pipeline-evidence-verification.md — evidence は自己申告不可)。
await agent(
  `EVIDENCE PERSISTER (Issue #99)。以下の JSON をファイル .omc/logs/${runId}/ticket-precheck.json へ書き出せ（ディレクトリが無ければ作成。親ディレクトリは repo root の .omc/logs/）。内容はこの JSON をそのまま pretty-print (2-space indent) したもの:
${JSON.stringify({
    runTimestamp,
    runId,
    issue: 99,
    verdict: ticketPrecheck.verdict,
    reason: ticketPrecheck.reason,
    declared: ticketPrecheck.declared,
    changed: ticketPrecheck.changed,
    undeclared: ticketPrecheck.undeclared,
    missing: ticketPrecheck.missing,
    changedCrates: precheckSliceMetadata.changedCrates,
    diffKind: precheckSliceMetadata.diffKind,
    // treeHash: git write-tree 実測値 (空 diff / vacuous PASS 検出用)。欠損時は
    // 'unknown' (fail-closed 規約 — 欠損を黙って green 払いしない)。
    treeHash: (precheckScope && typeof precheckScope.treeHash === 'string' && precheckScope.treeHash.length > 0)
      ? precheckScope.treeHash
      : 'unknown',
  }, null, 2)}
書き出し後、書き込んだファイルのパスのみを返せ。`,
  { label: 'request:persist-ticket-precheck', phase: 'Request', model: 'sonnet' // P-005: haiku は GLM backend で Unknown Model 400 (sonnet へ) }
);
if (ticketPrecheck.verdict !== 'PASS') {
  // fail-closed short-circuit (resolveReleaseAbort / evidence-failed と同型)。
  // Estimate 以降 (Implement / Gate / Release) を一切実行しない。実 diff は
  // working tree / commit に残るため ticket.files を修正して再実行すれば
  // 再開可能 (resumable status)。
  log(`Ticket precheck FAIL (Issue #99): ${ticketPrecheck.reason}`);
  return {
    status: 'precheck-failed',
    reason: ticketPrecheck.reason,
    ticket,
    precheck: {
      verdict: ticketPrecheck.verdict,
      declared: ticketPrecheck.declared,
      changed: ticketPrecheck.changed,
      undeclared: ticketPrecheck.undeclared,
      missing: ticketPrecheck.missing,
      sliceMetadata: precheckSliceMetadata,
    },
    requiredFixes: [
      `ticket-declared files が実 diff と不一致 (Issue #99): ${ticketPrecheck.reason}。ticket.files を実 diff に一致させて pipeline を再実行すること。`,
    ],
  };
}
log(`Ticket precheck PASS (Issue #99): declared ${ticketPrecheck.declared.length} file(s) match changed files; slice metadata crates=[${changedCratesList || 'none'}] diffKind=${diffKind} (precheck-derived)`);

// [issue-premise-wiring-begin]
// Issue #109 Task 2: issue premise precheck (Request→Estimate 間)。stale
// (closed かつ trunk merged issue) / duplicate (open PR 既存) を機械検出して
// Estimate 以降の dispatch を拒否する。evidence 収集は gh / git コマンドで、
// 判定は canonical ticket-precheck.js evaluateIssuePremise の inline copy
// (drift は tests/ticket-precheck-wiring.test.mjs / drift test が guard)。
// fail-closed: gh 認証失敗・rate limit で evidence が欠損した場合は
// malformed input として FAIL verdict (fail-open は stale 検出を素通りさせる)。
// (evaluateIssuePremise inline copy は前方 [ticket-premise] ブロック内に一元化済み —
//  Task 1/Task 2 並列実装による二重宣言を解消)
// evidence collector: gh issue view + git branch --contains (trunk membership)
// + gh pr list --search。各コマンド失敗時は null を返す (fail-closed)。
const issueNumberForPremise = ticket.issueNumber;
const premiseEvidence = issueNumberForPremise ? await agent(
  `ISSUE-PREMISE PROBE (Issue #109)。issue #${issueNumberForPremise} の前提検証 evidence を以下のコマンドで収集せよ:
1. \`gh issue view ${issueNumberForPremise} --json state,closedAt\` — issueState には state ("open"|"closed") を、closedAt も記録せよ。コマンド失敗 (認証/rate limit/存在しない) 場合は state を null とせよ (fail-closed)。
2. issue 番号を含むブランチの trunk membership:
   a. \`git branch -a | grep -E '(${issueNumberForPremise})'\` で該当ブランチ名を列挙
   b. 各ブランチ b について \`git branch -a --contains b\` を実行し、origin/master (又は remotes/origin/master) が含まれるか判定
   c. 一つでも origin/master に到達するブランチがあれば linkedBranchesContainIssue=true、無い・ブランチ自体無ければ false。列挙失敗時は null とせよ (fail-closed)。
3. \`gh pr list --search '${issueNumberForPremise} in:body' --state open --json number,title\` — 結果配列を openPRs へ。失敗時は null とせよ (fail-closed)。
**最後に StructuredOutput({issueState: "open"|"closed"|null, closedAt: "...", linkedBranchesContainIssue: true|false|null, openPRs: [{number, title}]|null}) を呼ぶこと。**`,
  { schema: { type: 'object', required: ['issueState'], properties: {
    issueState: { type: ['string', 'null'] },
    closedAt: { type: ['string', 'null'] },
    linkedBranchesContainIssue: { type: ['boolean', 'null'] },
    openPRs: { type: ['array', 'null'], items: { type: 'object', properties: { number: { type: 'number' }, title: { type: 'string' } } } },
  } }, label: 'request:issue-premise-evidence', phase: 'Request', model: 'sonnet' }
) : null;
// fail-closed: evidence 欠損 (issueNumber 無し / collector 失敗 / null フィールド)
// はそのまま pure fn へ流し malformed として FAIL 判定させる。
const issuePremise = evaluateIssuePremise({
  issueState: premiseEvidence && premiseEvidence.issueState,
  linkedBranchesContainIssue: premiseEvidence && premiseEvidence.linkedBranchesContainIssue,
  openPRs: premiseEvidence && premiseEvidence.openPRs,
});
// evidence persistence: verdict を .omc/logs/${runId} の
// .omc/logs/{runId}/issue-premise-precheck.json へ永続化
// (pipeline-evidence-verification.md — evidence は自己申告不可)。
await agent(
  `EVIDENCE PERSISTER (Issue #109)。以下の JSON をファイル .omc/logs/${runId}/issue-premise-precheck.json へ書き出せ（ディレクトリが無ければ作成。親ディレクトリは repo root の .omc/logs/）。内容はこの JSON をそのまま pretty-print (2-space indent) したもの:
${JSON.stringify({
    runTimestamp,
    runId,
    issue: 109,
    issueNumber: issueNumberForPremise || null,
    verdict: issuePremise.verdict,
    reason: issuePremise.reason,
    stale: issuePremise.stale,
    duplicate: issuePremise.duplicate,
    evidence: premiseEvidence || 'unavailable (fail-closed)',
  }, null, 2)}
書き出し後、書き込んだファイルのパスのみを返せ。`,
  { label: 'request:persist-issue-premise-precheck', phase: 'Request', model: 'sonnet' // P-005: haiku は GLM backend で Unknown Model 400 (sonnet へ) }
);
if (issuePremise.verdict !== 'PASS') {
  // fail-closed short-circuit (precheck-failed と同型の resumable status)。
  // Estimate 以降 (Implement / Gate / Release) を dispatch しない。
  log(`Issue premise precheck FAIL (Issue #109): ${issuePremise.reason}`);
  return {
    status: 'precheck-failed',
    reason: issuePremise.reason,
    ticket,
    issuePremise: {
      verdict: issuePremise.verdict,
      stale: issuePremise.stale,
      duplicate: issuePremise.duplicate,
      reason: issuePremise.reason,
    },
    requiredFixes: [
      `issue 前提検証 FAIL (Issue #109): ${issuePremise.reason}。stale なら issue を再オープンするか新規 ticket で再実行し、duplicate なら既存 open PR の継続として ticket を作り直すこと。`,
    ],
  };
}
log(`Issue premise precheck PASS (Issue #109): issue #${issueNumberForPremise} is not stale/duplicate — ${issuePremise.reason}`);
// [issue-premise-wiring-end]
// [ticket-precheck-wiring-end]

// ── Phase 2: Estimate ──
phase('Estimate');
const estimate = await agent(
  `You are the Tech Lead. Review this feature request and create a technical estimate:

Ticket: ${JSON.stringify(ticket)}

## Slice metadata (ticket-precheck 実測値 — Issue #99, 自己申告不可)
ticketPrecheck verdict=PASS。この slice の実 diff 由来メタデータ:
- changedCrates: ${JSON.stringify(precheckSliceMetadata.changedCrates)}
- diffKind: ${precheckSliceMetadata.diffKind}
- declared files: ${JSON.stringify(ticketPrecheck.declared)}
estimate の affectedCrates / tasks[].files はこの実測値と整合させること。

Analyze the codebase to understand the impact:
1. Which crates/modules are affected?
2. What files need to be changed?
3. What new files need to be created?
4. Are there dependency risks?
5. What is the estimated complexity (S/M/L/XL)?

Read the relevant source files to verify your assumptions.
Provide a detailed estimate with task breakdown.

## metadata.lane (必須 — Issue #99 Task 4 / Issue #106)

各 task に metadata.lane を必ず宣言すること (未宣言は lane-missing で fail-closed):
- "tdd": コード・テスト・ドキュメントを新規実装・改修するタスク (既定の実装テンプレート)
- "release": 未 push コミットの PR 化・push・PR 作成などリリース工程のタスク
- "merge": 未マージブランチ/Open PR の検証・merge・close 判定のタスク (実態調査型)

If you noticed issues with the workflow, role clarity, or tooling during this task,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [tech-lead] [category: workflow|tooling|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.`,
  { label: 'tech-lead:estimate', phase: 'Estimate', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      complexity: { type: 'string', enum: ['S', 'M', 'L', 'XL'] },
      affectedCrates: { type: 'array', items: { type: 'string' } },
      tasks: { type: 'array', items: { type: 'object', properties: {
        id: { type: 'string' },
        description: { type: 'string' },
        files: { type: 'array', items: { type: 'string' } },
        dependencies: { type: 'array', items: { type: 'string' } },
        metadata: { type: 'object', properties: {
          lane: { type: 'string', enum: ['tdd', 'release', 'merge'] },
        }, required: ['lane'] },
      }, required: ['id', 'description', 'files', 'metadata']}},
      risks: { type: 'array', items: { type: 'string' } },
    },
    required: ['complexity', 'affectedCrates', 'tasks'],
  }}
);
log(`Estimate: ${estimate.complexity} complexity, ${estimate.tasks.length} tasks`);

const approval = await agent(
  `You are the CTO. Review this estimate and decide APPROVE/REJECT/DEFER:

Feature: ${JSON.stringify(ticket)}
Estimate: ${JSON.stringify(estimate)}

Check:
1. Is the architecture sound? (coupling 原則は .claude/rules/architecture-coupling-balance.md 参照、bounded-context 例は anaden-helper の crate 階層に読み替え)
2. Are dependencies compatible? (Cargo.toml workspace members と実際の windows/scrcpy/image クレート版数で確認)
3. Is the task breakdown reasonable?
4. Are risks adequately addressed?

Provide your decision with rationale.

If you noticed issues with the workflow, role clarity, or tooling during this task,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [cto] [category: workflow|tooling|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.`,
  { label: 'cto:approve', phase: 'Estimate', model: 'opus', schema: {
    type: 'object',
    properties: {
      decision: { type: 'string', enum: ['APPROVE', 'REJECT', 'DEFER'] },
      rationale: { type: 'string' },
      conditions: { type: 'array', items: { type: 'string' } },
      feedback: { type: 'string' },
    },
    required: ['decision', 'rationale'],
  }}
);
log(`CTO Decision: ${approval.decision} - ${approval.rationale}`);

if (approval.decision === 'REJECT') {
  return { status: 'rejected', reason: approval.rationale, ticket, estimate, approval };
}
// P-003 (2026-08-21): DEFER は「条件付き再提出要求」— Implementation を実行せず
// ここで停止する。cycle-15 では DEFER 判定が REJECT 分岐をすり抜けて空コミット
// PR (#64) まで到達し Release Review 0/3 NO-GO で阻止された (issue #63)。
// DEFER の conditions を issue コメントに残し、人間/次サイクルの再提出判断に供する。
if (approval.decision === 'DEFER') {
  log(`CTO DEFER: Implementation をスキップ (条件付き再提出要求)`);
  try {
    await agent(
      `GitHub issue #${ticket.issueNumber} に CTO の DEFER コメントを追記せよ (gh issue comment 使用):\n` +
      `「CTO DEFER 判定 (feature-pipeline): 実装に進まず条件付き再提出を要求する。\n\n理由:\n${approval.rationale}\n\n` +
      `再提出条件:\n${(approval.conditions || []).map((c, i) => `${i + 1}. ${c}`).join('\n')}\n\n` +
      `このコメントはパイプラインの approval ゲート (P-003) により自動追記された。」`,
      { label: 'defer-comment' }
    );
  } catch (_e) {
    // コメント失敗は DEFER 停止自体には影響させない (org-feedback に記録のみ)
    log('DEFER コメント追記に失敗 (issue 手動確認推奨)');
  }
  return {
    status: 'deferred',
    reason: approval.rationale,
    conditions: approval.conditions || [],
    ticket,
    estimate,
    approval,
  };
}

// [implement-lane-begin]
// Issue #99 Task 4: Implementation Engineer テンプレート分岐。
// merge 前提タスク (未マージブランチ/Open PR 検証・コミット整理) に TDD 実装
// テンプレートを機械適用しない。判定は estimate task の metadata.lane
// ('release'|'merge') で明示的に行う — description パターンマッチは
// operator-gated 判定で起きた偽陽性と同種の問題を招くため禁止。
// fail-closed: lane metadata 欠損・不正値は TDD テンプレートへ黙墜ちせず
// タスクを 'lane-missing' として報告する (estimate approval 条件)。
// Pure block — unit-tested by .claude/workflows/tests/implement-lane-template.test.mjs
// (same marker-extraction pattern as done-evidence #68).
const IMPLEMENT_LANE_INVESTIGATION = new Set(['release', 'merge']);
const IMPLEMENT_LANE_TDD = 'tdd';
const resolveImplementLane = (task) => {
  const t = (task && typeof task === 'object') ? task : null;
  const meta = (t && t.metadata && typeof t.metadata === 'object' && !Array.isArray(t.metadata))
    ? t.metadata
    : null;
  if (!meta || !('lane' in meta)) {
    return { mode: 'fail-closed', reason: 'missing-lane-metadata' };
  }
  const lane = meta.lane;
  if (lane === IMPLEMENT_LANE_TDD) {
    // 明示的な TDD 宣言 — 従来の実装テンプレート
    return { mode: 'tdd', lane };
  }
  if (typeof lane !== 'string' || !IMPLEMENT_LANE_INVESTIGATION.has(lane)) {
    return { mode: 'fail-closed', reason: 'unknown-lane', lane };
  }
  // 'release' | 'merge' → 実態調査型 (PR 状態確認・trunk-membership・merge/close 判定)
  return { mode: 'investigation', lane };
};
const buildEngineerPrompt = (laneResult, task, ticket, approval) => {
  if (!laneResult || laneResult.mode === 'fail-closed') return null;
  if (laneResult.mode === 'investigation') {
    return `You are an Implementation Engineer (investigation lane: ${laneResult.lane}). This task is merge-oriented: verify the actual state of unmerged branches / open PRs and decide merge/close. Do NOT write new code or tests.

Task: ${JSON.stringify(task)}
Feature: ${JSON.stringify(ticket)}
Estimate Approval: ${JSON.stringify(approval)}

Investigate and report (evidence = raw command output, per .claude/rules/pipeline-evidence-verification.md):
1. PR 状態確認: gh pr list / gh pr view <n> --json state,mergeable,mergeCommit,title — 対象 PR の state・mergeability・checks
2. trunk-membership 検証: git branch -a --contains <merge-commit-sha> で origin/master 到達を確認
3. merge/close 判定: 上記 evidence に基づき、merge 済みなら issue/PR の close 可否を、未マージなら残作業 (コミット整理・rebase・conflict 解消) を特定
4. コミット整理: 未マージブランチのコミット履歴を確認し、squash/reorder の要否を報告

Do NOT commit, push, or merge. Only investigate and report findings with raw evidence.
If you noticed issues with the workflow, role clarity, or tooling during this task,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [engineer] [category: workflow|tooling|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.`;
  }
  // mode: 'tdd' — 従来の TDD 実装テンプレート
  return `You are an Implementation Engineer. Implement this task using TDD:

Task: ${JSON.stringify(task)}
Feature: ${JSON.stringify(ticket)}
Estimate Approval: ${JSON.stringify(approval)}

Follow the agent prompt template from .claude/rules/agent-prompt-template.md.
Follow TDD: Write tests FIRST, then implement.

IMPORTANT:
1. Read the relevant source files to verify API signatures BEFORE writing code
2. Add #[allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)] to test modules
3. Never use unwrap/expect/panic in library code
4. Run: cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo nextest run --workspace

Do NOT commit. Only implement and verify tests pass.

If you noticed issues with the workflow, role clarity, or tooling during this task,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [engineer] [category: workflow|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.`;
};
// [implement-lane-end]

// ── Phase 3: Implement ──
phase('Implement');
const implResults = await pipeline(
  estimate.tasks,
  async (task) => {
    // C: operator-gated task（capture/probe/live frame/ゲーム操作 等）は
    //    executor agent では実行不可（人間によるゲーム操作・キャプチャ取得が必要）。
    //    human-in-loop としてスキップし、ステータスを human-gated で報告。
    const _desc = ((task && task.description) || '').toLowerCase();
    // operator-gated 判定: 物理的ゲーム操作/実機キャプチャ「取得」が必須のタスクのみスキップ。
    // "probe skip pattern" / "capture backend" 等、コード修正タスクがパターン名で言及するだけの
    // 偽陽性を弾くため、(a) 物理/operator 操作シグナル AND (b) コード変更シグナルでない、を条件とする
    // （直前の失敗: BLOCKER FIX タスクが "probe skip pattern" に引っかかりスキップされた）。
    const _operatorAction = /(live\s*game|park(?:ed)?\s*game|operator\s*(?:runs?|drives?|must|capture)|physical\s*device|手動(?:による)?(?:の)?キャプチャ|実機(?:プローブ|デバイス|操作)|ゲーム(?:画面)?(?:へ?の?)?操作|navigate.*title.*screen|取得.*title_pc_probe|capture.*title_pc_probe)/i.test(_desc);
    const _codeChange = /(fix|refactor|add\s+(?:a\s+)?test|assert|absence-skip|\.rs\b|pipeline\.rs|contract\s+test|cargo\s+(?:nextest|clippy|fmt|check))/i.test(_desc);
    if (_operatorAction && !_codeChange) {
      log(`[human-in-loop] task ${task.id}: operator-gated → executor skip`);
      return {
        task: task.id,
        status: 'human-gated',
        reason: 'operator/capture task — 人間によるゲーム操作・キャプチャ取得が必要。executor agent は実行不可。Approve時にBLOCKER/外部依存として扱うこと。',
      };
    }
    // Issue #99 Task 4: TDD テンプレートと実態調査型プロンプトの分岐。
    // 判定は task.metadata.lane の明示値のみ (パターンマッチ禁止 — operator-gated
    // 判定で起きた偽陽性と同種の問題を避ける)。fail-closed: lane 欠損・不正値は
    // TDD テンプレートへ黙墜ちせず 'lane-missing' で報告 (estimate approval 条件)。
    const laneResult = resolveImplementLane(task);
    if (laneResult.mode === 'fail-closed') {
      log(`[lane-gate] task ${task.id}: ${laneResult.reason} → fail-closed (no dispatch)`);
      return {
        task: task.id,
        status: 'lane-missing',
        reason: `${laneResult.reason} — estimate task に metadata.lane ('release'|'merge'|'tdd') が未宣言か不正。TDD テンプレートへの黙墜ち防止 (Issue #99)。`,
      };
    }
    return agent(
      buildEngineerPrompt(laneResult, task, ticket, approval),
      { label: `engineer:${task.id}`, phase: 'Implement', model: 'sonnet' }
    );
  }
);
log(`Implementation: ${implResults.filter(Boolean).length}/${estimate.tasks.length} tasks completed`);

// ── R7: atomic snapshot commit (preserve implementation BEFORE the gate) ──
// Implement は「Do NOT commit」でコードを書くだけ。commit は Phase 5 Release が gate 後に
// 行うため、gate が stall/incomplete すると未コミット WIP が作業ブランチ上で行方不明に
// なる(R7 — 本セッション run wmvxllox5 で gate-incomplete→手動救出 を発端)。
// ここで gate 前に feature branch へ snapshot commit し、gate 成否に関わらず実装を保全する。
// 非破壞: try/catch で囲み失敗しても workflow は従来通り継続。master は触らない。
const _slug = ((ticket && ticket.title) || 'feature')
  .toLowerCase()
  .replace(/[^a-z0-9]+/g, '-')
  .replace(/^-+|-+$/g, '')
  .slice(0, 50);
const snapshotBranch = `feat/${_slug || 'feature'}`;
try {
  await agent(
    `SNAPSHOT (R7 work preservation). Commit the current implementation to a feature branch BEFORE the gate, so it is not lost as uncommitted WIP if the gate stalls. **Do NOT touch master** (no push either — push is Release's job).
1. git checkout -b ${snapshotBranch}  (if it already exists: git checkout ${snapshotBranch})
2. git add -A — but EXCLUDE tooling/local files: .claude.old/ .omc/ .understand-anything/ *.local *.lnk (stage only the feature's source changes)
3. git commit — let the pre-commit gate run; if it passes, commit normally with message "feat(snapshot): ${(ticket && ticket.title) || 'implementation'}". If it fails on WIP, retry with --no-verify -m "WIP(snapshot): ${(ticket && ticket.title) || 'implementation'}" so the snapshot is still preserved.
Final line MUST be exactly: SNAPSHOT_BRANCH=${snapshotBranch}`,
    { label: 'snapshot:r7-commit', phase: 'Implement', model: 'sonnet' }
  );
  log(`R7 snapshot: implementation preserved on ${snapshotBranch}`);
} catch (e) {
  log(`R7 snapshot: skipped (non-fatal, workflow continues): ${e}`);
}

// ── Phase 4: Commit Gate (6-dimension non-functional review) ──
phase('Commit Gate');

// ── S2 (Issue #68 Task 3): evidence-collector — Done 判定を主張でなく構造化 evidence に紐付ける ──
// Gate 直前に fresh-checkout (worktree) で cargo build --workspace --all-targets と
// cargo nextest run --workspace --all-targets を1回強制実行し、その exit code /
// pass 数を evidence として全 reviewer レーンへ注入する。evidence 欠落・build 赤・
// test 赤は fail-closed（Done 判定不可）。org-feedback 構造カテゴリB
// 「DoDが主張単位」の根源解消。
// ── Issue #68 (S2) Task 1: Done-evidence pure block ──
// Done 判定を主張でなく構造化 evidence に紐付ける (org-feedback 構造カテゴリB
// 「DoDが主張単位」の根治)。Gate 直前に fresh-checkout で
// cargo build --workspace --all-targets + cargo nextest run --workspace --all-targets
// を強制実行し、この pure 関数で機械的に判定する。fail-closed: evidence 欠落や
// 非 zero exit は一切 Done と認めない。
// Pure block — unit-tested by .claude/workflows/tests/done-evidence.test.mjs
// (same marker-extraction pattern as release-precheck #66).
// [done-evidence-begin]
const DONE_EVIDENCE_MAX_TAIL = 2000;
const isNonNegInt = (v) => Number.isInteger(v) && v >= 0;
// 返り値: { done, reason, evidenceSummary }
//  - done=true: 全 evidence が揃い build/test 成功 & checkout clean
//  - done=false + reason: 'missing-evidence' | 'missing-run-timestamp' | 'build-failed' | 'tool-missing' | 'tests-failed' | 'dirty-checkout'
//    判定順序 (precedence): missing > missing-run-timestamp > build > tool-missing > tests > dirty — より上流の障害を優先。
//    Task 3: tool-missing (cargo-nextest 不在, exit 127 等) は tests-failed と区別する
//    (estimate approval condition — reason の曖昧性排除)。
// Issue #97 UC-3: runTimestamp 欠損は 'missing-run-timestamp' で fail-closed。
// **collectedAt との使い分け (二重管理ではない)**: collectedAt = evidence agent が
// build/test を採取した時刻 (freshness 判定に使用)。runTimestamp = pipeline run の
// 開始時刻 (run metadata 由来。「どの run の evidence か」の traceability に使用)。
// collector は runTimestamp を自前生成せず run metadata から継承するため直交する関心。
const evaluateDoneEvidence = (input) => {
  const e = (input && typeof input === 'object') ? input : {};
  const missing = () => ({ done: false, reason: 'missing-evidence', evidenceSummary: 'evidence missing or malformed (fail-closed)' });
  if (typeof e.buildExitCode !== 'number' || typeof e.nextestExitCode !== 'number') return missing();
  if (!isNonNegInt(e.passed) || !isNonNegInt(e.failed)) return missing();
  // skipped は 0 でも null/undefined でも許容 (スキップ0件は正当な状態)
  const skipped = (e.skipped === null || e.skipped === undefined) ? 0 : e.skipped;
  if (!isNonNegInt(skipped)) return missing();
  if (typeof e.checkoutClean !== 'boolean') return missing();
  if (typeof e.collectedAt !== 'string' || e.collectedAt.length === 0) return missing();
  if (typeof e.runTimestamp !== 'string' || e.runTimestamp.length === 0) {
    return { done: false, reason: 'missing-run-timestamp', evidenceSummary: 'runTimestamp (pipeline run start time, Issue #97 UC-3) missing — traceability broken, fail-closed' };
  }
  if (e.buildExitCode !== 0) {
    return { done: false, reason: 'build-failed', evidenceSummary: `fresh-checkout build failed (exit=${e.buildExitCode})` };
  }
  if (e.toolMissing === true) {
    return { done: false, reason: 'tool-missing', evidenceSummary: `evidence tool unavailable (${e.tool || 'unknown'}) — tests NOT executed (distinct from tests-failed)` };
  }
  if (e.nextestExitCode !== 0 || e.failed > 0) {
    return { done: false, reason: 'tests-failed', evidenceSummary: `nextest failed (exit=${e.nextestExitCode}, passed=${e.passed}, failed=${e.failed})` };
  }
  if (e.checkoutClean !== true) {
    return { done: false, reason: 'dirty-checkout', evidenceSummary: 'checkout not clean after build+test run' };
  }
  return {
    done: true,
    reason: null,
    evidenceSummary: `build exit=0, nextest exit=0, passed=${e.passed}, failed=0, skipped=${skipped}, checkout clean (${e.collectedAt})`,
  };
};
// formatEvidenceForReviewers: reviewer 全レーンの prompt に注入する平文 blob。
// R8 GATE_DIFF と並んで注入されるため outputTail は DONE_EVIDENCE_MAX_TAIL 文字で cap。
// evidence 欠落時も「欠落している」事実を明示 (reviewer が静かに pass しないよう)。
// Issue #97 Task 3: 第2引数 runTimestamp は run 開始時刻 (UC-1)。欠損時は
// 'unknown' を明示 (UC-3 fail-closed — collectedAt は採取時刻で別物, 役割分離)。
const formatEvidenceForReviewers = (evidence, runTimestamp) => {
  const runTs = (typeof runTimestamp === 'string' && runTimestamp.trim() !== '')
    ? runTimestamp
    : 'unknown';
  if (!evidence || typeof evidence !== 'object') {
    return `## Done Evidence\nrunTimestamp=${runTs}\nSTATUS: NOT COLLECTED (missing evidence — Done判定不可, fail-closed)`;
  }
  const v = evaluateDoneEvidence(evidence);
  let tail = '';
  if (typeof evidence.outputTail === 'string' && evidence.outputTail.length > 0) {
    const t = evidence.outputTail.slice(-DONE_EVIDENCE_MAX_TAIL);
    tail = `\noutput tail (last ${DONE_EVIDENCE_MAX_TAIL} chars):\n${t}`;
  }
  return `## Done Evidence (fresh-checkout, structured — Issue #68)
runTimestamp=${runTs}
STATUS: ${v.done ? 'DONE (evidence-backed)' : `NOT DONE (${v.reason})`}
build exit=${String(evidence.buildExitCode)}, nextest exit=${String(evidence.nextestExitCode)}
passed=${String(evidence.passed)}, failed=${String(evidence.failed)}, skipped=${String(evidence.skipped ?? 0)}
checkoutClean=${String(evidence.checkoutClean)}, collectedAt=${String(evidence.collectedAt)}
runTimestamp=${(typeof evidence.runTimestamp === 'string' && evidence.runTimestamp.length > 0) ? evidence.runTimestamp : 'unknown'} (pipeline run start time — Issue #97 UC-3; collectedAt は採取時刻, runTimestamp は run 開始時刻)${tail}`;
};
// [done-evidence-end]

// [gate-evidence-begin]
// Pure evidence logic, unit-tested by
// .claude/workflows/tests/gate-evidence.test.mjs (TDD, Issue #68 Task 3).
// R7 snapshot branch を stranded にしないため、evidence 失敗時は resumable な
// 構造化 status 'evidence-failed' で short-circuit する（precheck-failed と同型）。
const GATE_EVIDENCE_STATUS_FAILED = 'evidence-failed';
const GATE_DIFF_EMPTY_STATUS = 'diff-empty-failed';
const GATE_EVIDENCE_SCHEMA = {
  type: 'object',
  properties: {
    buildExitCode: { type: 'number' },
    nextestExitCode: { type: 'number' },
    passed: { type: 'number' },
    failed: { type: 'number' },
    checkoutClean: { type: 'boolean' },
    collectedAt: { type: 'string' },
    // Issue #97 UC-3: runTimestamp = pipeline run 開始時刻 (run metadata 由来)。
    // collectedAt (採取時刻) とは直交 — freshness は collectedAt、traceability は runTimestamp。
    runTimestamp: { type: 'string' },
    toolMissing: { type: 'boolean' },
    tool: { type: 'string' },
    outputTail: { type: 'string' },
  },
  required: ['buildExitCode', 'nextestExitCode', 'checkoutClean', 'collectedAt', 'runTimestamp'],
};
// structured object / fenced-or-bare JSON 文字列の両方を受容し、数値を検証して
// 正規化する。不正入力は null（fail-closed: lane MISSING 相当）。
const normalizeGateEvidence = (result) => {
  let obj = null;
  if (result && typeof result === 'object' && !Array.isArray(result)) {
    obj = result;
  } else if (typeof result === 'string' && result.length > 0) {
    const fenced = result.match(/```(?:json)?\s*(\{[\s\S]*?\})\s*```/);
    const bare = fenced ? null : result.match(/\{[\s\S]*?"buildExitCode"[\s\S]*?\}/);
    const candidate = fenced ? fenced[1] : bare ? bare[0] : null;
    if (candidate) {
      try { obj = JSON.parse(candidate); } catch { obj = null; }
    }
  }
  if (!obj || typeof obj !== 'object') return null;
  const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v
    : (typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v)) ? Number(v) : null));
  const buildExitCode = num(obj.buildExitCode);
  const nextestExitCode = num(obj.nextestExitCode);
  if (buildExitCode === null || nextestExitCode === null) return null;
  const passed = num(obj.passed);
  const failed = num(obj.failed);
  return {
    buildExitCode,
    nextestExitCode,
    passed: passed === null ? null : passed,
    failed: failed === null ? null : failed,
    checkoutClean: obj.checkoutClean === true,
    collectedAt: typeof obj.collectedAt === 'string' ? obj.collectedAt : '',
    // Issue #97 UC-3: 欠損時は '' (evaluateDoneEvidence が 'missing-run-timestamp' で
    // fail-closed、formatEvidenceForReviewers は 'unknown' 表示 — NOT COLLECTED 相当)。
    runTimestamp: typeof obj.runTimestamp === 'string' ? obj.runTimestamp : '',
    // nextest/tool missing ≠ tests failed（曖昧性排除 — estimate approval condition）。
    // cargo-nextest 不在は exit 127 / "no such subcommand" 出力で現れる。
    toolMissing: obj.toolMissing === true,
    tool: typeof obj.tool === 'string' ? obj.tool : null,
    outputTail: typeof obj.outputTail === 'string' ? obj.outputTail : '',
  };
};
// [gate-evidence-end]

// evidence collector agent: snapshot branch の fresh worktree で build+nextest を
// 1回実行する（dirty working tree は使わない）。R7 snapshot 失敗時も working tree の
// HEAD 相当で実行するが、その場合は checkoutClean=false を報告させる。
const gateEvidenceRaw = await agent(
  `EVIDENCE COLLECTOR (S2, Issue #68). Commit Gate 用に fresh-checkout で build と test を1回ずつ実行し、構造化 evidence を返す。**dirty な working tree では実行しないこと** — 必ず新規 worktree を作る:
1. \`git worktree add ../gate-evidence-wt ${snapshotBranch}\`（branch が無い場合は \`git worktree add ../gate-evidence-wt HEAD\`）。Windows で lock エラーになる場合は \`git worktree add --no-checkout\` 後 checkout。
2. worktree 内 (../gate-evidence-wt) で実行: \`cargo build --workspace --all-targets\` → buildExitCode
3. 同じく実行: \`cargo nextest run --workspace --all-targets\` → nextestExitCode と passed/failed 件数（nextest サマリー行 "N passed; M failed" から抽出）
4. cargo-nextest が無い (exit 127 / "no such subcommand" / "command not found") 場合は toolMissing=true, tool='cargo-nextest' を報告（tests-failed と混同しないこと）
5. 終了後 \`git worktree remove ../gate-evidence-wt --force\`（best-effort、失敗しても続行）
【runTimestamp 照合 (Issue #97)】\`.omc/logs/${runId}/run-metadata.json\` を読み、その runTimestamp (run 開始時刻) を構造化 evidence の runTimestamp フィールドとして報告せよ（自前生成しないこと）。各 evidence には採取時刻 recordedAt (ISO8601) も設定せよ。run-metadata.json が無い・破損している・runTimestamp フィールドが無い場合は runTimestamp='unknown' を報告し、その evidence は検証不足 (fail-closed) として扱われる。欠損ディレクトリへの遡及補填はしない (Issue #97 の非スコープ)。
最後に StructuredOutput で {buildExitCode, nextestExitCode, passed, failed, checkoutClean, collectedAt, runTimestamp, toolMissing, tool, outputTail} を返す。checkoutClean は worktree 使用 + worktree 内で git status --porcelain が空なら true。collectedAt は ISO8601。outputTail は失敗時の最後 30 行程度（成功時は空文字可）。`,
  { label: 'gate:evidence', phase: 'Commit Gate', model: 'sonnet', schema: GATE_EVIDENCE_SCHEMA }
);
const gateEvidence = normalizeGateEvidence(gateEvidenceRaw);
const doneEvidence = evaluateDoneEvidence(gateEvidence);
log(`S2 gate evidence: done=${doneEvidence.done}${doneEvidence.reason ? ` reason=${doneEvidence.reason}` : ''}`);
const evidenceShortCircuit = !doneEvidence.done;
if (evidenceShortCircuit) {
  // fail-closed: evidence が無効なら reviewer を spawn しない。R7 snapshot branch は
  // 成立済みなので実装は失われない（stranded にしない — resumable status）。
  log(`Commit Gate EVIDENCE-FAILED (${doneEvidence.reason}): Done 判定に必要な evidence が無効のため gate を短絡する (Issue #68 S2, fail-closed)。snapshot=${snapshotBranch}`);
  return {
    status: GATE_EVIDENCE_STATUS_FAILED,
    ticket, estimate, approval, implResults,
    snapshotBranch,
    evidence: gateEvidence,
    evidenceReason: doneEvidence.reason,
    requiredFixes: [
      `Evidence 失敗 (${doneEvidence.reason})。修正後 pipeline 再実行で再収集される。実装は ${snapshotBranch} に保全済み。`,
    ],
  };
}
const GATE_EVIDENCE = formatEvidenceForReviewers(gateEvidence, runTimestamp);

// [run-metadata-prompt-check-begin]
// Issue #97 Task 2: Commit Gate / Release Review / evidence collector の各 agent
// プロンプトへ注入する共通の runTimestamp 照合指示。runTimestamp は pipeline run の
// 開始時刻 (.omc/logs/{runId}/run-metadata.json 由来) であり、evidence の
// recordedAt/collectedAt (採取時刻) と照合して「どの run の evidence か」を検証する。
// メタデータ欠損ディレクトリ (run-metadata.json 無し / JSON 破損 / runTimestamp
// フィールド無し) は unknown として fail-closed 扱い。遡及補填はしない
// (Issue #97 の非スコープ — 照合可能なのは新規 run のみ)。
const RUN_TS_CHECK = `【runTimestamp 照合 (Issue #97)】.omc/logs/${runId}/run-metadata.json の runTimestamp を読み、
evidence の recordedAt (採取時刻) と照合せよ。runTimestamp = run 開始時刻、recordedAt = 採取時刻であり
両者は役割が異なる (二重管理ではない)。run-metadata.json が無い・JSON が破損している・runTimestamp
フィールドが無い場合は unknown として扱い、その evidence は検証不足 (fail-closed) として GO 判定の
根拠にしないこと。メタデータ欠損ディレクトリへの遡及補填はしない (Issue #97 の非スコープ)。`;
// [run-metadata-prompt-check-end]

// 【Issue #63 Task 4 — Commit Gate の TeamCreate 接続 (wired, capability-gated)】
// gate 6レーンを TeamCreate teammate として並列化する経路を下記 runCommitGateViaTeam
// で接続した。詳細仕様は ./team-gate-protocol.js の PROTOCOL_SPEC
// (node --test team-gate-protocol.test.mjs / tests/team-gate-wiring.test.mjs で検証)。
// activation は capability-gated: ハーネスが team primitives を公開しない場合は
// 従来の `parallel()` + StructuredOutput 経路 (R8 diff-inject / R9 opus retry /
// R6 text fall-back) へ透過的に fall-back する (Task 1 probe の実測: サブエージェント
// サンドボックスから teammate spawn 不可 — org-feedback 2026-08-21)。
// 元 DEFER 条件の帰結:
//   1. Task 1 (probe) 実施済み: primitives は呼べるが teammate 起動は top-level
//      セッション必須 → capability-gated 構造を採用 (fall-back が正経路として常設)
//   2. 30% ウォールクロック短縮は計測されず → 並列化自体は既存 parallel() が達成。
//      team 経路は structured verdict (JSON envelope first) の付加価値経路として接続
//   3. verdict 伝達は構造化 (JSON envelope) 維持 — プレーンテキスト parse は R6 最終フォールバックのみ
//   4. gate-incomplete 時 release block / consensus semantics は不変 (既存ロジックを再利用)

// ── R8 (large-diff gate robustness): diff-inject ──
// 6人の sonnet レビュアーがそれぞれ独立に working tree を再探索すると、大diff
// (≥1500行) でターン予算をファイル読出に費やし StructuredOutput 呼出に到達
// できない → gate-incomplete（検証: runs wkjg81zgu/wr3v60rky, Issue #37 T4/T5,
// ~1800行 diff で6次元全滅）。diff を1回 agent 取得して全レビュアーへ埋め込み、
// 「探索」を「提供済み diff の直接分析」へ変える。小diff(#39等)では影響なし。
// Issue #91 (P-007) T2: fetch は commit-range diff (HEAD~1..HEAD / merge-base)
// と tree hash も収集し、working-tree 空 diff 時は commit-range へ fallback。
// 両方空なら fail-closed (vacuous GO 防止)。helpers は review-gate-diff.js の
// canonical copy を inline (Workflow runtime は ESM import を reject するため
// script は self-contained 必須 — review-gate.js と同じ構造、drift は
// tests/gate-commit-range-diff.test.mjs が正準比較で guard)。
function extractDiffSection(raw, name) {
  if (typeof raw !== 'string' || raw === '') return '';
  const startMarker = `=== ${name} ===`;
  const start = raw.indexOf(startMarker);
  if (start < 0) return '';
  const bodyStart = start + startMarker.length;
  const next = raw.indexOf('===', bodyStart);
  const body = next < 0 ? raw.slice(bodyStart) : raw.slice(bodyStart, next);
  return body.trim();
}
function buildCommitRangeDiffInput(input, options = {}) {
  const str = (v) => (typeof v === 'string' ? v : '');
  const stat = str(input.stat);
  const diff = str(input.diff);
  const untracked = str(input.untracked);
  const rangeStat = str(input.rangeStat);
  const rangeDiff = str(input.rangeDiff);
  const rangeVariant = options.rangeVariant ?? 'head-prev';
  const out = { stat, diff, untracked };
  if (input.treeHash != null) out.treeHash = input.treeHash;
  if (diff.trim() !== '' || stat.trim() !== '') {
    out.mode = 'working-tree';
    return out;
  }
  if (rangeDiff.trim() !== '' || rangeStat.trim() !== '') {
    out.mode = 'commit-range';
    out.stat = rangeStat;
    out.diff = rangeDiff;
    out.rangeVariant = rangeVariant;
    return out;
  }
  out.mode = 'fail-closed';
  out.reason = 'working-tree diff and commit-range diff are both empty';
  if (untracked.trim() !== '') {
    out.note = 'untracked files present but no tracked diff: gate must not emit a vacuous GO; '
      + 're-run with `git add -N` (intent-to-add) diff or enumerate files for individual Read';
  }
  return out;
}
const diffFetch = await agent(
  `Diff fetcher for the Commit Gate (Issue #91 P-007). In the repo CWD run these and return ONE plain-text string (no JSON, no commentary wrapper):
1. \`git --no-pager diff HEAD --stat\`
2. \`git --no-pager diff HEAD\`
3. \`git --no-pager status --porcelain\`
4. \`git --no-pager diff HEAD~1..HEAD --stat\` (commit-range; HEAD~1 が解決不能な merge context の場合は代わりに \`git --no-pager diff $(git merge-base origin/master HEAD)..HEAD --stat\` を使う)
5. \`git --no-pager diff HEAD~1..HEAD\` (上と同じ merge-base fallback)
6. \`git write-tree\` (tree hash — 「green だが実体は空」検出用)
Concatenate with headers "=== STAT ===", "=== DIFF ===", "=== UNTRACKED ===", "=== COMMIT-RANGE STAT ===", "=== COMMIT-RANGE DIFF ===", "=== TREE HASH ===". If the DIFF or COMMIT-RANGE DIFF section exceeds 28000 chars, emit full STAT + UNTRACKED + the other sections but only the FIRST 24000 chars of the oversized DIFF section, then a line "[DIFF TRUNCATED]". Return ONLY the concatenated text.`,
  { label: 'gate:fetch-diff', phase: 'Commit Gate' }
);
const GATE_DIFF_RAW = (typeof diffFetch === 'string'
  ? diffFetch
  : (diffFetch == null ? '' : JSON.stringify(diffFetch))
).slice(0, 30000);
// [gate-commit-range-diff-begin]
// Issue #91 (P-007) T2: working-tree diff が空 (commit 済み slice) の場合は
// commit-range diff (HEAD~1..HEAD / merge-base) へ fallback し、tree hash を
// evidence に添付する。両方空なら fail-closed — 空の diff を reviewer に注入して
// vacuous GO させることはしない (pipeline-evidence-verification.md §2,
// S2 evidence fail-closed パターンと同一構造)。
const gateDiffInput = buildCommitRangeDiffInput({
  stat: extractDiffSection(GATE_DIFF_RAW, 'STAT'),
  diff: extractDiffSection(GATE_DIFF_RAW, 'DIFF'),
  untracked: extractDiffSection(GATE_DIFF_RAW, 'UNTRACKED'),
  rangeStat: extractDiffSection(GATE_DIFF_RAW, 'COMMIT-RANGE STAT'),
  rangeDiff: extractDiffSection(GATE_DIFF_RAW, 'COMMIT-RANGE DIFF'),
  treeHash: extractDiffSection(GATE_DIFF_RAW, 'TREE HASH') || null,
});
if (gateDiffInput.mode === 'fail-closed') {
  // fail-closed: 空 diff を reviewer に注入しない。R7 snapshot branch は成立済み
  // なので実装は失われない (evidence-failed と同じ短絡構造)。
  log(`Commit Gate DIFF-EMPTY-FAILED: ${gateDiffInput.reason}${gateDiffInput.note ? ` note=${gateDiffInput.note}` : ''} — gate を短絡する (Issue #91 P-007, fail-closed)。snapshot=${snapshotBranch}`);
  return {
    status: GATE_DIFF_EMPTY_STATUS,
    ticket, estimate, approval, implResults,
    snapshotBranch,
    diffInput: gateDiffInput,
    requiredFixes: [
      `diff が空 (${gateDiffInput.reason})。commit 済み slice の場合は commit-range diff (HEAD~1..HEAD) が自動使用されるが、それも空だった。対象コミットが存在するかスライス範囲を確認し pipeline 再実行のこと。実装は ${snapshotBranch} に保全済み。`,
    ],
  };
}
const GATE_DIFF = [
  // Issue #97 Task 3: reviewer が diff を読む際、どの run (runTimestamp/runId) の
  // evidence か追跡できるよう冒頭に run メタデータを注入。
  `=== RUN TIMESTAMP === ${runTimestamp} (runId=${runId})`,
  '=== STAT ===',
  gateDiffInput.stat,
  '=== DIFF ===',
  gateDiffInput.diff,
  '=== UNTRACKED ===',
  gateDiffInput.untracked,
  ...(gateDiffInput.treeHash ? ['=== TREE HASH ===', gateDiffInput.treeHash] : []),
].join('\n').slice(0, 30000);
log(`Commit Gate: injected diff context (${GATE_DIFF.length} chars) into all reviewers (R8)`);

// [gate-diff-kind-persist-begin]
// Issue #95 (P-008) T3c: short-circuit 判定根拠の永続化。Evidence は自己申告不可
// (.claude/rules/pipeline-evidence-verification.md) — diff 分類 (classification)、
// 根拠ファイル (basisFiles)、分類対象 tree の識別子 (treeHash) を
// .omc/logs/{run-id}/diff-kind-short-circuit.json へ書き出し、機械検証可能にする。
// treeHash は gate:fetch-diff が採取した git write-tree 値 (working-tree の構成を
// 一意に特定 — 「どの diff が分類されたか」の追跡性)。
// Issue #97 Task 3: runId/runTimestamp は run 開始時の共有値を使用し
// .omc/logs/{runId}/ (run-metadata.json と同一ディレクトリ) へ書き出す —
// 全永続 JSON から runTimestamp が参照可能 (UC-2 照合)。
// recordedAt は採取時刻、runTimestamp は run 開始時刻 (役割分離)。
// NOTE: ACTIVE_GATE_DIMENSIONS は GATE_DIMENSIONS (ticket 依存) の後方で宣言される
// ため、rationale JSON の構築をここで実行すると TDZ 違反。builder として定義し、
// ACTIVE_GATE_DIMENSIONS 確定直後 ([gate-diff-kind-persist-apply]) で構築・永続化する。
const buildDiffKindRationale = () => ({
  recordedAt: runTimestamp, // Date API 禁制のため run 開始時刻を流用 (採取時刻の厳密分離は諦め)
  runTimestamp,
  runId,
  issue: 95,
  classification: diffKind,
  shortCircuited: diffKind === 'docs-only',
  activeLanes: ACTIVE_GATE_DIMENSIONS.map((d) => d.key),
  skippedLanes: diffKind === 'docs-only' ? DOCS_ONLY_SKIP_KEYS : [],
  basisFiles: changedFilesArr,
  treeHash: gateDiffInput.treeHash,
  classifier: 'gate-diff-kind.js classifyDiffKind (inline copy, drift-guarded by tests/gate-diff-kind-wiring.test.mjs)',
});
// NOTE: builder の呼び出し (agent 永続化を含む) は ACTIVE_GATE_DIMENSIONS が
// 確定する後方の [gate-diff-kind-persist-apply] に移動済み (TDZ 違反修正)。

// [gate-contrast-begin]
// Issue #91 (P-007) T5: intent<->fact contrast check。ticket の意図 (title,
// design/tasks declared files) と実際の diff (facts) の整合を機械的に検査し、
// 結果を全 reviewer prompt (FEEDBACK_INSTRUCTION 経由) に注入する。
// mismatch は advisory: reviewer は CONDITIONAL + 明示的 override 根拠を要求
// (hard NO-GO ではない — CEO 承認ポイントでの override 可能)。
// helpers は review-gate-contrast.js の canonical copy を inline (Workflow
// runtime は ESM import を reject — tests/gate-contrast-wiring.test.mjs が
// 正準比較 + 振る舞い contract で drift guard)。
function extractChangedPaths(diff) {
  const paths = new Set();
  if (typeof diff !== 'string') return [...paths];
  const gitLines = diff.matchAll(/^diff --git a\/(\S+) b\/(\S+)$/gm);
  for (const m of gitLines) { paths.add(m[1]); paths.add(m[2]); }
  const plusLines = diff.matchAll(/^\+\+\+ b\/(\S+)$/gm);
  for (const m of plusLines) paths.add(m[1]);
  const statLines = diff.matchAll(/^\s+(\S+?)\s+\|\s+\d+/gm);
  for (const m of statLines) {
    if (!m[1].startsWith('...') && m[1] !== '') paths.add(m[1]);
  }
  return [...paths];
}
function contrastTitleKeywords(title) {
  if (typeof title !== 'string') return [];
  return title.toLowerCase().split(/[^a-z0-9]+/).filter((w) => w.length >= 3);
}
function contrastPathMatches(path, kw) {
  const p = path.toLowerCase();
  return p.includes(kw) || path.split('/').pop().toLowerCase().includes(kw);
}
function testContrast({ ticketTitle, designFiles = [], taskFiles = [], diff }) {
  const mismatches = [];
  const changed = extractChangedPaths(diff);
  const kws = contrastTitleKeywords(ticketTitle);
  const declared = [...designFiles, ...taskFiles].filter((f) => typeof f === 'string');
  const overlapsTitle =
    changed.length > 0 &&
    (changed.some((p) => kws.some((kw) => contrastPathMatches(p, kw))) ||
      changed.some((p) => declared.includes(p)));
  if (!overlapsTitle) {
    mismatches.push({
      kind: 'title-diff',
      detail: changed.length === 0
        ? 'diff is empty — no changed files to contrast against ticket intent (fail-closed signal)'
        : `changed files [${changed.join(', ')}] do not overlap ticket title keywords [${kws.join(', ')}]`,
    });
  }
  if (declared.length > 0 && changed.length > 0) {
    const declaredBasenames = declared.map((f) => f.split('/').pop().toLowerCase());
    const overlapsDeclared = changed.some(
      (p) => declared.includes(p) || declaredBasenames.includes(p.split('/').pop().toLowerCase())
    );
    if (!overlapsDeclared) {
      mismatches.push({
        kind: 'design-tasks',
        detail: `changed files [${changed.join(', ')}] do not overlap design/tasks declared files [${declared.join(', ')}]`,
      });
    }
  }
  return { consistent: mismatches.length === 0, mismatches };
}
const gateContrastTaskFiles = (Array.isArray(estimate.tasks) ? estimate.tasks : [])
  .flatMap((t) => (t && Array.isArray(t.files) ? t.files : []));
const gateContrast = testContrast({
  ticketTitle: ticket.title,
  designFiles: [],
  taskFiles: gateContrastTaskFiles,
  diff: GATE_DIFF,
});
const GATE_CONTRAST_REPORT = gateContrast.consistent
  ? '【INTENT<->FACT CONTRAST (P-007)】整合: 実diff は ticket 意図 (title keywords / declared files) と重複している。mismatch なし。'
  : `【INTENT<->FACT CONTRAST (P-007) — MISMATCH 検出】下記の intent<->fact mismatch を確認せよ:
${gateContrast.mismatches.map((m) => `- [${m.kind}] ${m.detail}`).join('\n')}
mismatch が妥当な理由 (例: 工夫されたスライス構成・declared files の過剰包含) を reviewer が説明でき、
かつその説明が findings に明示された場合のみ GO 可能。説明できない mismatch がある場合は
**CONDITIONAL** を返し、summary に「contrast mismatch override に必要な追加確認事項」を明記すること。
mismatch を無視して無条件 GO に*しない*こと (override には明示的正当化が必須 — CEO 承認ポイント)。`;
log(`Commit Gate contrast (P-007): ${gateContrast.consistent ? 'consistent' : `MISMATCH (${gateContrast.mismatches.map((m) => m.kind).join(', ')})`} — mismatch がある場合 reviewer は CONDITIONAL + override 根拠を要求`);
// [gate-contrast-end]

const V_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['GO', 'NO-GO', 'CONDITIONAL'] },
    dimension: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          severity: { type: 'string', enum: ['critical', 'major', 'minor', 'info', 'praise'] },
          title: { type: 'string' },
          detail: { type: 'string' },
          evidence: { type: 'string' },
          suggestion: { type: 'string' },
        },
        required: ['severity', 'title', 'detail'],
      },
    },
    summary: { type: 'string' },
  },
  required: ['verdict', 'dimension', 'findings', 'summary'],
};

const FEEDBACK_INSTRUCTION = `

${RUN_TS_CHECK}

【提供済み diff で直接評価（R8 — large-diff gate robustness）】下記に working-tree の完全 diff を示す。
これを直接読んで分析すること。ファイルを個別に Read して再取得*しない*こと（diff が手元にある）。
あなたの次元に特有の grep/check は1回まで許可するが、tool 使用は最小限にし、最終アクションは
*必ず* StructuredOutput を呼ぶこと（prose-only で終わらせない）。

=== WORKING-TREE DIFF ===
${GATE_DIFF}
=== END DIFF ===

${GATE_CONTRAST_REPORT}

${GATE_EVIDENCE}
// [gate-evidence-inject-begin]
// Task 4 (Issue #68 S2): 上記 evidence blob を全 reviewer レーン（team 経路の
// teammate prompt・fall-back parallel() 経路の d.prompt、双方 FEEDBACK_INSTRUCTION 経由）
// に注入し、Done/完了 判定は evidence 引用を必須とする旨を指示する。
// evidence 欠落は fail-closed（evaluateDoneEvidence が gate 手前で短絡済み —
// ここに到達する時点で evidence は有効）。
【Done/完了 判定は evidence 引用必須（Issue #68 S2）】上記の fresh-checkout
build/test evidence（exit code・pass 数）を確認せよ。「実装が完了している」「Done」
等の完了系の主張・GO 判定は、この evidence を引用して根拠を示すことが必須である。
evidence に反する完了主張（例: build 赤なのに Done と述べる）は検証不足として
NO-GO 扱いにすること。evidence の数値が示す失敗が本スライス由来であれば findings に
file:line 付きで記録せよ（baseline 起票ルールは既存の two-layer gate 準拠）。
// [gate-evidence-inject-end]

【Gate の評価対象（重要）】Commit Gate は working tree のコード変更（品質・正確性・副作用）を評価する。
staging（git add）状態は Release フェーズの責務であり、Gate は Release 前に走るため
「ファイルが staged でない」「未追跡ファイルがコミットに含まれない」を理由に NO-GO に**しないこと**。
Release フェーズが force-track/staging/commit を実行する。Gate は『コード変更自体の品質』のみ判定し、
staging の完全性（tracked/untracked）は Release Review（3レビュアー）で検証する。

If you noticed issues with the workflow, role clarity, or tooling during this review,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [gate-reviewer:{dimension}] [category: workflow|tooling|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.

CRITICAL: Your verdict is delivered ONLY by calling the StructuredOutput tool with
{verdict, dimension, findings, summary}. You MUST call StructuredOutput as your final
action. Do NOT end your turn with a prose-only summary — if no tool is called, the
review is recorded as MISSING and blocks the release.`;

// P-008 (Issue #95): lane 定義。docs-only diff では ACTIVE_GATE_DIMENSIONS が
// reliability/performance/extensibility を落とす (governance + integration 常時維持)。
const GATE_DIMENSIONS = [
  { key: 'reliability', prompt: `RELIABILITY REVIEWER. Evaluate error handling, edge cases, and fault tolerance of the changes.

Feature: ${ticket.title}
Changed files（git SoT・実diff）: ${changedFilesList}

Check:
1. Error paths: do all fallible operations return Result?
2. Edge cases: empty input, EOF, recursion limits
3. Panic safety: grep for unwrap/expect/panic in library code (not tests)
4. Resource safety: no infinite loops, buffer consistency

Read the changed files and verify. Return verdict: GO/NO-GO/CONDITIONAL.${FEEDBACK_INSTRUCTION}` },
  { key: 'performance', prompt: `PERFORMANCE REVIEWER. Evaluate performance impact of the changes.

Feature: ${ticket.title}

Check:
1. No new dynamic dispatch (vtables, dyn Trait) in hot paths
2. No unnecessary allocations (String where &str works, clone where borrow works)
3. Inlining not affected by visibility changes
4. Run: cargo check --all (verify compile time not degraded)

Return verdict: GO/NO-GO/CONDITIONAL.${FEEDBACK_INSTRUCTION}` },
  { key: 'extensibility', prompt: `EXTENSIBILITY REVIEWER. Evaluate future-proofing and maintainability.

Feature: ${ticket.title}
Changed files（git SoT・実diff）: ${changedFilesList}

Check:
1. Module cohesion: are related methods grouped properly?
2. Adding similar features later: is it easy?
3. Visibility: pub/pub(crate)/pub(super) correct?
4. Naming: self-documenting names?
5. Judgment calls: any gray areas in naming/design decisions?

Return verdict: GO/NO-GO/CONDITIONAL.${FEEDBACK_INSTRUCTION}` },
  { key: 'governance', prompt: `GOVERNANCE REVIEWER. Evaluate documentation and traceability.

Feature: ${ticket.title}

Check:
1. Public APIs have doc comments (/// or //!).
2. No stale doc comments after changes.
3. Commit granularity: single commit or multiple?
4. Architecture docs: does this change require doc updates? Check .claude/rules/ files.
5. Public API stability: re-exports unchanged?

Return verdict: GO/NO-GO/CONDITIONAL.${FEEDBACK_INSTRUCTION}` },
  { key: 'security', prompt: `SECURITY REVIEWER. Evaluate safety of the changes.

Feature: ${ticket.title}

Check:
1. Grep for "unsafe" in changed files. Any found?
2. Recursion depth limits preserved?
3. Input validation: does malformed input cause panics?
4. Grep for unwrap/expect/panic in library code (not #[cfg(test)]).
5. No new dependencies introduced?

Run: grep -rn "unsafe\\|unwrap()\\|expect(\\|panic!" in changed files.
Return verdict: GO/NO-GO/CONDITIONAL.${FEEDBACK_INSTRUCTION}` },
  { key: 'integration', prompt: `INTEGRATION REVIEWER. Evaluate downstream impact with the TWO-LAYER GATE (R5: per-slice / baseline 分離).

Feature: ${ticket.title}
このスライスが触った crates (git SoT): ${changedCratesList || '(none)'}

【背景】workspace-wide gate(cargo clippy/nextest --workspace -D warnings)は、このスライスが触っていない
crate の pre-existing baseline 負債(他スライスや未追跡の clippy・test 赤)まで同梱してしまい、クリーンな
feature が無関係な負債でブロックされる(org-feedback 36/40/47/66/109)。
生の workspace-wide 赤を信じて NO-GO に**しないこと**。失敗を「このスライスが導入したか」「baseline 負債か」で属性判定せよ。

Step 1 — per-slice 層(このスライスの責務):
  ${integrationSliceCheckText()}

Step 2 — baseline-attribution 層(baseline 負債はブロックしない):
  workspace-wide 実行(cargo clippy/nextest --workspace)で、スライスが触って**いない** crate に失敗があれば、
  それは BASELINE DEBT(どのスライスの責務でもない)。これでこの release をブロック**しないこと**。代わりに:
  - 各 baseline 失敗(test name / file:line)を baseline-owned として記録。
  - gh issue create で追跡 issue を起票(label: baseline-debt) — track-todos-as-github-issues.md に従い「忘れない状態」にする。
  - 報告に baseline-debt リスト(起票した issue 番号付き)を含める。

Step 3 — downstream 影響:
  - cargo check --all: 全 crate コンパイル可能か?(baseline コンパイル赤は Step2 同様ブロックしないが記録)
  - Public API: downstream consumer 向け unchanged か?(触った crate の pub 署名 diff で判定)
  - Cross-module 依存: 新規 coupling 導入なしか?

verdict は「per-slice 層(Step1)」の結果で出す(GO/NO-GO)。baseline-debt は verdict に関わらず報告する。
証拠(テスト件数, コマンド出力, baseline-debt issue 番号)を添えること。${FEEDBACK_INSTRUCTION}` },
];

// P-008 (Issue #95): 分類結果 (diffKind, Coordinate フェーズで算出) を lane 構成へ反映。
// GATE_DIMENSIONS がここで確定するため ACTIVE_GATE_DIMENSIONS の計算もこの位置。
// fail-closed: フィルタ結果が空になることは GATE_DIFF_KIND_LANES が GATE_DIMENSIONS
// 全体へ fallback するため構造的にない (governance + integration は docs-only でも維持)。
const ACTIVE_GATE_DIMENSIONS = GATE_DIFF_KIND_LANES();
const activeLaneCount = ACTIVE_GATE_DIMENSIONS.length;
// [gate-diff-kind-persist-apply]
// TDZ 修正: buildDiffKindRationale (前方で定義した builder) を ACTIVE_GATE_DIMENSIONS
// 確定後に呼び出して rationale JSON を構築し、haiku persister で永続化する。
const diffKindRationale = buildDiffKindRationale();
await agent(
  `EVIDENCE PERSISTER (Issue #95 P-008 T3c)。以下の JSON をファイル .omc/logs/${runId}/diff-kind-short-circuit.json へ書き出せ（ディレクトリが無ければ作成。親ディレクトリは repo root の .omc/logs/）。内容はこの JSON をそのまま pretty-print (2-space indent) したもの:
${JSON.stringify(diffKindRationale, null, 2)}
書き出し後、書き込んだファイルのパスのみを返せ。`,
  { label: 'gate:persist-diff-kind-rationale', phase: 'Commit Gate', model: 'sonnet' // P-005: haiku は GLM backend で Unknown Model 400 (sonnet へ) }
);
log(`Diff-kind (P-008): short-circuit rationale persisted to .omc/logs/${runId}/diff-kind-short-circuit.json (classification=${diffKind}, lanes=${activeLaneCount}, treeHash=${gateDiffInput.treeHash || 'n/a'})`);
// [gate-diff-kind-persist-apply-end]
if (diffKind === 'docs-only') {
  log(`Diff-kind (P-008): docs-only diff — short-circuiting ${DOCS_ONLY_SKIP_KEYS.length} code-adjacent lane(s); ${activeLaneCount} lane(s) remain (governance + integration). Reason: every changed path matches a docs pattern (classifyDiffKind fail-closed for empty/unknown).`);
} else {
  log(`Diff-kind (P-008): ${diffKind} diff — full ${GATE_DIMENSIONS.length}-lane gate.`);
}

// [gate-team-begin]
// Pure verdict-normalization and aggregation logic for the TeamCreate Commit Gate
// path (Issue #63 Task 4). Extracted as a marker-delimited block and unit-tested by
// .claude/workflows/tests/team-gate-wiring.test.mjs (TDD).
//
// Transport precedence (PROTOCOL_SPEC — structured-first):
//   1. structured object (StructuredOutput / TaskUpdate metadata)
//   2. fenced or bare JSON envelope in a string body (GATE_VERDICT_JSON)
//   3. R6 lenient keyword text parse — LAST RESORT only
// Anything else → null → lane stays MISSING → gate-incomplete refusal (invariant).
const GATE_TEAM_VERDICT_ENUM = ['GO', 'NO-GO', 'CONDITIONAL'];
const GATE_TEAM_COERCE = (raw) => {
  if (typeof raw !== 'string') return null;
  const upper = raw.toUpperCase();
  if (upper === 'NOGO') return 'NO-GO';
  return GATE_TEAM_VERDICT_ENUM.includes(upper) ? upper : null;
};
const normalizeGateReview = (result, dimension) => {
  // Path 1: already-structured object.
  if (result && typeof result === 'object' && !Array.isArray(result)) {
    const verdict = GATE_TEAM_COERCE(result.verdict);
    if (!verdict) return null;
    return {
      verdict,
      dimension: typeof result.dimension === 'string' && result.dimension ? result.dimension : dimension,
      findings: Array.isArray(result.findings)
        ? result.findings.filter((f) => f && typeof f === 'object' && f.severity && f.title && f.detail)
        : [],
      summary: typeof result.summary === 'string' ? result.summary : '',
    };
  }
  if (typeof result !== 'string' || result.length === 0) return null;
  // Path 2: JSON envelope (fenced ```json / bare object containing "verdict").
  const fenced = result.match(/```(?:json)?\s*(\{[\s\S]*?\})\s*```/);
  const bare = fenced ? null : result.match(/\{[\s\S]*"verdict"[\s\S]*\}/);
  const candidate = fenced ? fenced[1] : bare ? bare[0] : null;
  if (candidate) {
    try {
      const obj = JSON.parse(candidate);
      const verdict = GATE_TEAM_COERCE(obj.verdict);
      if (verdict) {
        return {
          verdict,
          dimension: typeof obj.dimension === 'string' && obj.dimension ? obj.dimension : dimension,
          findings: Array.isArray(obj.findings)
            ? obj.findings.filter((f) => f && typeof f === 'object' && f.severity && f.title && f.detail)
            : [],
          summary: typeof obj.summary === 'string' ? obj.summary : '',
        };
      }
    } catch { /* fall through to R6 */ }
  }
  // Path 3 (LAST RESORT): R6 lenient keyword parse. NO-GO/NOGO tested before GO
  // so a "NO-GO" never false-matches GO; \b prevents GOOD/GOING false positives.
  const m = result.match(/\b(?:VERDICT:\s*)?(NO-GO|NOGO|CONDITIONAL|GO)\b/i);
  if (!m) return null;
  return {
    verdict: GATE_TEAM_COERCE(m[1]),
    dimension,
    findings: [],
    summary: `(R6 text fall-back — 構造化 envelope 欠如のため verdict を parse)\n${result.slice(-1500)}`,
  };
};
// Deterministic aggregate (pre-verdict for the CONSENSUS JUDGE). Semantics
// identical to the in-pipeline gate: any missing lane → INCOMPLETE (never
// synthesize from partial evidence); ANY NO-GO → NO-GO; all GO → GO; else CONDITIONAL.
const aggregateGateReviews = (reviews, expectedDimensions = 6) => {
  // Issue #95 (P-008) T3: only the first `expectedDimensions` entries are
  // "expected" lanes. Entries beyond the expected count were intentionally
  // short-circuited away (docs-only) and must NOT count as MISSING — a stale
  // full-length array would false-fire gate-incomplete.
  const list = (Array.isArray(reviews) ? reviews : []).slice(0, expectedDimensions);
  const present = list.filter(Boolean);
  const missing = list.map((r, i) => (r == null ? i : -1)).filter((i) => i >= 0);
  if (present.length + missing.length < expectedDimensions || missing.length > 0) {
    return { preVerdict: 'INCOMPLETE', complete: false, blocking: [], missing, goCount: present.filter((r) => r.verdict === 'GO').length, noGoCount: present.filter((r) => r.verdict === 'NO-GO').length };
  }
  const noGo = present.filter((r) => r.verdict === 'NO-GO');
  const goCount = present.filter((r) => r.verdict === 'GO').length;
  if (noGo.length > 0) {
    return { preVerdict: 'NO-GO', complete: true, blocking: noGo.map((r) => r.dimension), missing: [], goCount, noGoCount: noGo.length };
  }
  if (goCount === expectedDimensions) {
    return { preVerdict: 'GO', complete: true, blocking: [], missing: [], goCount, noGoCount: 0 };
  }
  return { preVerdict: 'CONDITIONAL', complete: true, blocking: [], missing: [], goCount, noGoCount: 0 };
};
// [gate-team-end]

// TeamCreate-based Commit Gate (Issue #63 Task 4, capability-gated wiring).
// Returns the 6 normalized gateReviews array, or null when the team path is
// unavailable/failed (caller then runs the existing parallel() route unchanged).
const gateTeamPrimitivesAvailable = () =>
  typeof TeamCreate === 'function' &&
  typeof SendMessage === 'function' &&
  typeof TaskCreate === 'function' &&
  typeof TaskList === 'function' &&
  typeof TaskUpdate === 'function';
const runCommitGateViaTeam = async () => {
  if (!gateTeamPrimitivesAvailable()) {
    log('Commit Gate (team): team primitives unavailable in this harness — falling back to parallel() path');
    return null;
  }
  const teamName = `commit-gate-${runId}`; // Date API 禁制のため runId を使用
  try {
    await TeamCreate({ team_name: teamName, description: `Commit Gate for issue #${ticket.issueNumber || 'none'}` });
  } catch (e) {
    log(`Commit Gate (team): TeamCreate failed (${e}) — falling back to parallel() path`);
    return null;
  }
  try {
    // Fan out the 6 dimension prompts as teammates. R8 diff-inject is already
    // embedded in each dimension prompt; the teammate protocol demands a
    // GATE_VERDICT_JSON envelope reply (structured-first).
    const spawnResult = await agent(
      `TEAM SPAWNER。team "${teamName}" に6名の gate reviewer teammate を追加せよ。
各 teammate へ以下のプロンプトを割り当てること（Agent tool, team_name: "${teamName}"）:
${ACTIVE_GATE_DIMENSIONS.map((d) => `--- teammate gate-${d.key} へのプロンプト ---\n${d.prompt}\n\n【teammate プロトコル】レビュー完了後、自分の task を TaskUpdate で completed にし metadata に {verdict, dimension, findings, summary} を設定せよ。加えて team-lead 宛 SendMessage で GATE_VERDICT_JSON envelope を必ず送ること（構造化 verdict が第一経路 — prose-only はレビュー消失として扱われる）。`).join('\n')}
報告: 追加した teammate 名のリスト。`,
      { phase: 'Commit Gate', label: 'gate:team-spawn', model: 'sonnet' }
    );
    // Poll TaskList until all 6 lanes are completed (structured metadata) or timeout.
    // Date API 禁制のため反復回数ベースの cap (15s interval × 80 = 20 min 相当)。
    const MAX_POLLS = 80;
    const collected = new Array(ACTIVE_GATE_DIMENSIONS.length).fill(null);
    for (let poll = 0; poll < MAX_POLLS; poll++) {
      const tasks = await TaskList();
      for (const t of (tasks || [])) {
        if (!t) continue;
        const idx = ACTIVE_GATE_DIMENSIONS.findIndex(
          (d, i) => (t.metadata && (t.metadata.dimension === d.key || t.metadata.laneIndex === i)) || (t.subject || '').includes(`gate-${d.key}`)
        );
        if (idx < 0 || collected[idx] != null) continue;
        if (t.status === 'completed') {
          collected[idx] = normalizeGateReview(t.metadata || null, ACTIVE_GATE_DIMENSIONS[idx].key);
        }
      }
      if (collected.every((v) => v != null)) break;
      await new Promise((r) => setTimeout(r, 15000));
    }
    const agg = aggregateGateReviews(collected, ACTIVE_GATE_DIMENSIONS.length);
    log(`Commit Gate (team ${teamName}): ${agg.goCount}/${ACTIVE_GATE_DIMENSIONS.length} GO${agg.noGoCount > 0 ? `, ${agg.noGoCount} NO-GO` : ''}${!agg.complete ? `, ${collected.filter((v) => v == null).length} MISSING (block)` : ''} [spawn: ${String(spawnResult).slice(0, 200)}]`);
    return collected;
  } finally {
    try { await (typeof TeamDelete === 'function' ? TeamDelete({ team_name: teamName }) : null); } catch (_e) { /* best effort */ }
  }
};

let gateReviews;
let gateTeamAgg = null;
try {
  gateReviews = await runCommitGateViaTeam();
} catch (e) {
  log(`Commit Gate (team): unexpected failure (${e}) — falling back to parallel() path`);
  gateReviews = null;
}
if (gateReviews == null) {
  // Fallback: original parallel() + StructuredOutput route (R6-hardened, unchanged).

const runGateDimension = (d) =>
  agent(d.prompt, { label: `gate:${d.key}`, phase: 'Commit Gate', model: 'sonnet', schema: V_SCHEMA });

// R9 (gate schema-flakiness robustness): sonnet reviewers intermittently finish
// without calling StructuredOutput even with R8 diff-inject (verified run wnt9x6yy4:
// #42 small-diff shard, 4/6 sonnet reviewers null after retry+R6). opus (used by
// impl/CTO/issue-verify) reliably calls the tool in this harness. Retry null
// dimensions with opus before the text fallback. Sonnet-first keeps cost down on
// healthy runs; opus only on the failing minority.
const runGateDimensionOpus = (d) =>
  agent(d.prompt, { label: `gate-retry:${d.key}`, phase: 'Commit Gate', model: 'opus', schema: V_SCHEMA });

gateReviews = await parallel(ACTIVE_GATE_DIMENSIONS.map((d) => () => runGateDimension(d)));

// A reviewer that ends without calling StructuredOutput resolves to null.
// Retry each missing dimension once with a fresh agent so a flaky
// StructuredOutput call does not silently drop a review.
const missingAfterFirst = ACTIVE_GATE_DIMENSIONS
  .map((d, i) => (gateReviews[i] == null ? i : -1))
  .filter((i) => i >= 0);
if (missingAfterFirst.length > 0) {
  log(`Commit Gate: ${missingAfterFirst.length}/${ACTIVE_GATE_DIMENSIONS.length} reviewer(s) returned no verdict (${missingAfterFirst.map((i) => ACTIVE_GATE_DIMENSIONS[i].key).join(', ')}). Retrying once.`);
  const retried = await parallel(
    missingAfterFirst.map((i) => () => runGateDimensionOpus(ACTIVE_GATE_DIMENSIONS[i]))
  );
  missingAfterFirst.forEach((idx, k) => {
    gateReviews[idx] = retried[k];
  });
}

// ── R6 mitigation: text fall-back for schema-reluctant reviewers ──
// sonnet gate reviewers intermittently finish without calling StructuredOutput
// (R6 — strand verified-correct releases at gate-incomplete). When a dimension
// is still null after the retry, make ONE more attempt WITHOUT a schema, asking
// the reviewer to emit a final `VERDICT: GO|NO-GO|CONDITIONAL` line, then parse
// it into the minimal V_SCHEMA shape. No semantic change on healthy runs (the
// schema path is still preferred); strictly better than gate-incomplete.
// Verified 2026-07-04: rescued cycle-4 (PR #31) from gate-incomplete → merged.
const stillMissingAfterRetry = ACTIVE_GATE_DIMENSIONS
  .map((d, i) => (gateReviews[i] == null ? i : -1))
  .filter((i) => i >= 0);
if (stillMissingAfterRetry.length > 0) {
  log(`Commit Gate (R6 fall-back): ${stillMissingAfterRetry.length}/${ACTIVE_GATE_DIMENSIONS.length} reviewer(s) still null after retry (${stillMissingAfterRetry.map((i) => ACTIVE_GATE_DIMENSIONS[i].key).join(', ')}). Attempting text fall-back (no schema).`);
  const fallbackResults = await parallel(
    stillMissingAfterRetry.map((i) => () => {
      const d = ACTIVE_GATE_DIMENSIONS[i];
      return agent(
        `${d.prompt}\n\n【R6+R9 text fall-back (opus)】StructuredOutput ツールが呼べない場合のためのフォールバックです。レビューを実施した上で、回答の**どこか**（最終行を推奨）に次のいずれかを含めてください（VERDICT: プレフィックスは任意、大文字小文字問わない）:\nGO / NO-GO / CONDITIONAL\n（例: "全体として GO" / "verdict: no-go" / "CONDITIONAL: ...確認推奨" すべて受け付ける）`,
        { label: `gate-fallback:${d.key}`, phase: 'Commit Gate', model: 'opus' }
      ).then((text) => {
        if (!text) return null;
        // R9 lenient parse: accept verdict keyword anywhere, with or without VERDICT: prefix.
        // NO-GO/NOGO tested before GO to avoid false GO. \bGO\b won't match GOOD/GOING.
        const m = text.match(/\b(?:VERDICT:\s*)?(NO-GO|NOGO|CONDITIONAL|GO)\b/i);
        if (!m) return null;
        const raw = m[1].toUpperCase();
        return {
          verdict: raw === 'NOGO' ? 'NO-GO' : raw,
          dimension: d.key,
          findings: [],
          summary: `(R6+R9 opus text fall-back — schema 未呼出のため verdict を parse)\n${text.slice(-1500)}`,
        };
      }).then((parsed) => parsed, () => null);
    })
  );
  stillMissingAfterRetry.forEach((idx, k) => {
    if (fallbackResults[k] != null) gateReviews[idx] = fallbackResults[k];
  });
}
} // end fallback (parallel()) route — team route above fills gateReviews directly

// Unified semantics gate (both routes): compute the deterministic aggregate.
// The CONSENSUS JUDGE receives this pre-verdict in both paths (PROTOCOL_SPEC).
gateTeamAgg = aggregateGateReviews(gateReviews, ACTIVE_GATE_DIMENSIONS.length);

// Refuse to release on an incomplete gate rather than letting the consensus
// judge synthesize a GO from partial evidence (org-feedback 2026-06-17).
const missingDimensions = ACTIVE_GATE_DIMENSIONS
  .map((d, i) => (gateReviews[i] == null ? d.key : null))
  .filter(Boolean);
if (missingDimensions.length > 0) {
  log(`Commit Gate INCOMPLETE: ${missingDimensions.length}/${ACTIVE_GATE_DIMENSIONS.length} dimension(s) produced no verdict even after retry (${missingDimensions.join(', ')}). Refusing to release without a complete gate.`);
  return {
    status: 'gate-incomplete',
    ticket, estimate, approval, implResults,
    gate: { partialReviews: gateReviews, missingDimensions },
    requiredFixes: [
      `Re-run the commit gate; these reviewers returned no verdict after retry: ${missingDimensions.join(', ')}`,
    ],
  };
}

// Consensus Judge
const consensus = await agent(
  `CONSENSUS JUDGE. Synthesize 6 gate reviews into final GO/NO-GO.

Reviews:
${JSON.stringify(gateReviews, null, 2)}

Rules:
- ANY NO-GO with critical finding → NO-GO
- All GO → GO
- CONDITIONAL → GO if conditions are minor
- INTENT<->FACT CONTRAST (P-007) mismatch に由来する CONDITIONAL は、mismatch の妥当性が
  reviewer findings で明示的に正当化 (override justification) された場合のみ GO 可能。
  正当化なしの mismatch を伴う GO は出さないこと (mismatch は hard NO-GO ではなく
  CEO が override できる CONDITIONAL 扱い — Override reason を judgment_calls に記録)。
- BASELINE DEBT はブロック要因ではない: integration reviewer が baseline-debt issue 起票済みの
  pre-existing 赤(このスライスが触っていない crate 由来)で、かつ per-slice 層(Step1)が GO なら、
  その integration verdict は GO 扱い。baseline 負債の起票済み issue 番号を follow_up_items に記録。

Provide:
1. Verdict matrix (dimension | verdict | key finding)
2. Blocking issues (critical/major only, with evidence)
3. Judgment calls (topics with no single right answer, both sides, consensus)
4. Follow-up items (non-blocking, with priority)
5. Final verdict: GO or NO-GO
6. If GO: commit message (conventional commits) and body

If you noticed issues with the workflow, role clarity, or tooling during this judgment,
append a single line to .claude/org-feedback.md:
[YYYY-MM-DD] [consensus-judge] [category: workflow|tooling|role-ambiguity|bottleneck|suggestion] specific constructive feedback
Only append if you have genuine feedback. Be specific.`,
  { label: 'gate:consensus', phase: 'Commit Gate', model: 'opus', schema: {
    type: 'object',
    properties: {
      final_verdict: { type: 'string', enum: ['GO', 'NO-GO'] },
      commit_message: { type: 'string' },
      commit_body: { type: 'string' },
      blocking_issues: { type: 'array', items: { type: 'string' } },
      judgment_calls: { type: 'array', items: { type: 'object', properties: {
        topic: { type: 'string' },
        consensus: { type: 'string' },
      }}},
      follow_up_items: { type: 'array', items: { type: 'string' } },
    },
    required: ['final_verdict', 'commit_message', 'blocking_issues'],
  }}
);

log(`Commit Gate: ${consensus.final_verdict}`);

if (consensus.final_verdict === 'NO-GO') {
  return {
    status: 'gate-blocked',
    ticket, estimate, approval, implResults,
    // R7: 実装は gate 前に snapshot commit 済み（snapshotBranch）。NO-GO でも成果物は
    // この branch に保全されているため、requiredFixes 適用後に再開可能（未コミット WIP で消失しない）。
    snapshotBranch,
    gate: { reviews: gateReviews, consensus },
    requiredFixes: consensus.blocking_issues,
  };
}

// ── Phase 5: Release — mechanical empty-release precheck (Issue #66) ──
// 3回再発した空リリースサイクル (PR #64/#65, cycle-14 stale PR title) の根絶。
// LLM Release エージェントの prompt 任せではなく、workflow JS レベルで機械的に
// 「リリースすべき tracked 変更が存在するか」を判定してからエージェントを起動する。
// SoT は resolve-scope と同一 (git diff HEAD --name-only + git status --porcelain)。
// R7 誤検知ガード: snapshot commit が存在する場合は work は既に commit 済みなので
// empty diff でもアボートしない (Release エージェントが branch を push するだけ)。

// [release-precheck-begin]
// Pure precheck logic, unit-tested by
// .claude/workflows/tests/release-precheck.test.mjs (TDD, Issue #66 Task 1).
const RELEASE_EXCLUDE_PATTERNS = [
  /^\.claude\//, /^\.claude\.old\//, /^\.omc\//, /^\.understand-anything\//,
  /\.local$/i, /\.lnk$/i,
];
const isReleaseExcluded = (f) =>
  typeof f === 'string' && f.length > 0 && RELEASE_EXCLUDE_PATTERNS.some((re) => re.test(f));
// porcelain 1行 (e.g. " M src/a.rs", "M  b.rs", "?? c.txt") から tracked 変更パスを抽出。
// untracked ("?? ") は Release のステージ対象外 (除外パターン相当として扱わないが
// tracked 変更ゼロ判定の救済にもしない — untracked は R7 snapshot が .claude/ 等を
// 除外して add する運用と同様、空リリース防止のため対象外)。
const trackedPathFromPorcelain = (line) => {
  if (typeof line !== 'string' || line.length < 4) return null;
  if (line.startsWith('??')) return null;
  const p = line.slice(3).trim();
  return p.length > 0 ? p : null;
};
// 返り値: { abort, reason, staged, excluded, untrackedCount }
//  - abort=false: Release エージェントを起動してよい
//  - abort=true + reason='no-tracked-changes': tracked 変更ゼロ & snapshot なし
//  - abort=true + reason='exclusion-only-changes': 変更はあるが全て除外パターン
//  - abort=true + reason='gitignored-only-artifacts' (P-003/step-3, Issue #109):
//    hasSnapshotCommit=false かつ staged=0 かつ untracked 全部が gitignore 対象の
//    成果物パスの場合。成果物が gitignored で git diff --cached が空のまま
//    空 PR が出るのを hard-fail する。
const GITIGNORED_ARTIFACT_PATTERNS = [
  /^\.omc\//, /^target\//, /^dist\//, /^build\//, /^node_modules\//,
  /^coverage\//, /\.log$/i, /\.tmp$/i,
];
const isGitignoredArtifact = (f) =>
  typeof f === 'string' && f.length > 0 && GITIGNORED_ARTIFACT_PATTERNS.some((re) => re.test(f));
const untrackedPathFromPorcelain = (line) => {
  if (typeof line !== 'string' || line.length < 4) return null;
  if (!line.startsWith('??')) return null;
  const p = line.slice(3).trim();
  return p.length > 0 ? p : null;
};
const evaluateReleasePrecheck = (input) => {
  const { diffNames, porcelainLines, hasSnapshotCommit } = (input && typeof input === 'object')
    ? input
    : {};
  const names = new Set();
  let untrackedCount = 0;
  const untrackedPaths = [];
  if (Array.isArray(diffNames)) {
    for (const f of diffNames) if (typeof f === 'string' && f.length > 0) names.add(f);
  }
  if (Array.isArray(porcelainLines)) {
    for (const line of porcelainLines) {
      if (typeof line !== 'string' || line.length === 0) continue;
      if (line.startsWith('??')) {
        untrackedCount += 1;
        const u = untrackedPathFromPorcelain(line);
        if (u) untrackedPaths.push(u);
        continue;
      }
      const p = trackedPathFromPorcelain(line);
      if (p) names.add(p);
    }
  }
  const all = [...names];
  const staged = all.filter((f) => !isReleaseExcluded(f));
  const excluded = all.filter((f) => isReleaseExcluded(f));
  if (hasSnapshotCommit === true) {
    // R7 guard が最優先: snapshot commit があれば work は commit 済みのため
    // gitignored-only 判定を適用しない (空 diff 正常パスを壊さない — Issue #109 task 4)。
    return { abort: false, reason: null, staged, excluded, untrackedCount };
  }
  if (staged.length === 0
    && untrackedPaths.length > 0
    && untrackedPaths.every((f) => isGitignoredArtifact(f))) {
    return { abort: true, reason: 'gitignored-only-artifacts', staged, excluded, untrackedCount };
  }
  if (all.length === 0) {
    return { abort: true, reason: 'no-tracked-changes', staged, excluded, untrackedCount };
  }
  if (staged.length === 0) {
    return { abort: true, reason: 'exclusion-only-changes', staged, excluded, untrackedCount };
  }
  return { abort: false, reason: null, staged, excluded, untrackedCount };
};
// [release-precheck-end]

phase('Release');
// Issue #66: Release エージェント起動前の機械的プレチェック。SoT 収集は
// resolve-scope と同じ git コマンドをこの時点で再実行 (実装後の状態を取得)。
const releaseScope = await agent(
  `RELEASE PRECHECK SCOPE RESOLVER。以下を実行し結果を返す:
1. \`git diff HEAD --name-only\` — HEAD に対する変更ファイル(tracked)
2. \`git status --porcelain\` — 全ファイル状態(tracked+untracked)
3. 現在 branch が R7 snapshot branch 上で snapshot commit が存在するか:
   git rev-parse --abbrev-ref HEAD が \`feat/\` で始まり、かつ
   git log --oneline -1 に snapshot/WIP コミットがあれば hasSnapshotCommit=true。
**最後に StructuredOutput({diffNames: [...], porcelainLines: [...], hasSnapshotCommit: bool}) を呼ぶこと。**`,
  { schema: { type: 'object', required: ['diffNames', 'porcelainLines'], properties: {
    diffNames: { type: 'array', items: { type: 'string' } },
    porcelainLines: { type: 'array', items: { type: 'string' } },
    hasSnapshotCommit: { type: 'boolean' },
  } }, label: 'release:precheck-scope', phase: 'Release', model: 'sonnet' }
// P-005 (2026-08-21): model: 'sonnet' // P-005: haiku は GLM backend で Unknown Model 400 (sonnet へ) は GLM バックエンドで "Unknown Model" 400 になり
// agent が StructuredOutput を呼べず workflow 全体が throw した (cycle-18)。
// haiku 指定はこのハーネスで使用不可 — sonnet へ修正。
);
const precheck = evaluateReleasePrecheck({
  diffNames: releaseScope && releaseScope.diffNames,
  porcelainLines: releaseScope && releaseScope.porcelainLines,
  hasSnapshotCommit: releaseScope && releaseScope.hasSnapshotCommit === true,
});
// ── Issue #66 Task 3: empty-release abort status branch ──
// org-feedback.md 参照エントリ:
//   [2026-08-21] [tech-lead] empty-release 欠陥が3回再発、機械的プレチェックで構造的に防止 (issue #66)
//   [2026-08-21] [cto] JS 判定ロックは unit test で保護しないと静かに退化する (Issue #66)
// アボート時は Phase 6-7 (Release Review / Merge) をスキップし、Phase 8 Self-Improve
// へ直接進んだ上で return status 'empty-release-aborted' を返す。
// [release-abort-status-begin]
const RELEASE_ABORT_STATUS = 'empty-release-aborted';
const ABORT_TOKEN_RE = /\bABORTED\b/;
// resolveReleaseAbort: unify (a) the mechanical JS precheck (Task 1) and
// (b) the Release agent's own ABORTED report token (Task 2 prompt guard).
// Pure function — unit-tested by .claude/workflows/tests/release-abort-status.test.mjs.
// Fail-closed: either signal reporting an abort wins.
const resolveReleaseAbort = (input) => {
  const { precheck, releaseResult } = (input && typeof input === 'object') ? input : {};
  if (precheck && typeof precheck === 'object' && precheck.abort === true) {
    return { aborted: true, reason: precheck.reason || 'no-tracked-changes' };
  }
  if (typeof releaseResult === 'string' && ABORT_TOKEN_RE.test(releaseResult)) {
    return { aborted: true, reason: 'agent-reported-abort' };
  }
  return { aborted: false, reason: null };
};
// [release-abort-status-end]
let precheckAbort = null;
if (precheck.abort) {
  // 実装が失われたわけではない: R7 snapshot が無かった場合のみここに来るため、
  // working tree / 既存 branch はそのまま。再開情報として現在の状態を返す。
  // (Task 3: early return せず、Phase 6-7 をスキップして Self-Improve へ直接進む)
  precheckAbort = precheck;
  log(`Release PRECHECK ABORT (${precheck.reason}): tracked staged=0, excluded=[${precheck.excluded.join(', ')}], untracked=${precheck.untrackedCount} — empty release prevented (Issue #66)`);
}
log(`Release precheck (Issue #66): staged=${precheck.staged.length} excluded=${precheck.excluded.length} — proceeding to Release agent`);
// [release-prompt-begin]
// Issue #66 Task 2: Release エージェントのプロンプト自体にも防御手順を明記（二重防御）。
// Task 1 の JS プレチェックに加え、エージェント内でも commit 直前に
// git diff --cached --name-only を確認し、tracked 変更ゼロなら push/PR を中止する。
// R7 整合: snapshot commit が存在し差分がそれのみの場合は正常パス扱い。
// Unit-tested by .claude/workflows/tests/release-prompt-guard.test.mjs (TDD).
const buildReleasePrompt = (consensus, ticket, snapshotBranch) => `RELEASE MANAGER (Step 1: branch → commit → push → PR)。変更は Commit Gate (6次元 all GO) 通過済み。

【R7 事前 snapshot の可能性】Implement 直後の snapshot step が既に feature branch
\`${snapshotBranch}\` へ実装を commit 済みかもしれない（work preservation）。まず確認:
  git rev-parse --abbrev-ref HEAD と git log --oneline -1 で、現在 ${snapshotBranch} 上に
  snapshot commit があるか判定せよ。
- 既に commit 済みなら: 新規 commit せず、その branch を push + PR 作成のみ（手順 4-5 へ）。
  snapshot の WIP commit message を amend する場合は、**元 conventional-commit メッセージを保持したまま
  scope のみ修正**せよ（git commit --amend で scope 部分だけ編集）。
  type の書き換え (feat → feat(snapshot) 等)、body の削除、Co-Authored-By trailer の落としは全て禁止。
  --amend --no-edit（message 変更なし）も許容される。
- 未 commit（snapshot 失敗/未実行）なら: 手順 1-5 を従来通り実行。

【空リリース防止アサーション (Issue #66) — commit 実行直前に必ず実施】
手順 3 の commit を実行する**直前**に \`git diff --cached --name-only\` を実行して
staged な tracked 変更を確認せよ:
- staged ファイルが 1 つ以上 → 正常。そのまま commit/push/PR を続行。
- staged ファイルがゼロ（出力空）→ **アボート**: commit も push も PR 作成も中止し、
  レポートに \`ABORTED\` を明記せよ（空 PR + 不適切な Closes #N を防ぐ）。
  アボート時は現在の branch 名 (git rev-parse --abbrev-ref HEAD) と
  git status --short の概要（untracked 含む）をレポートに添えよ。
  実装は失われたわけではない（snapshot branch 上か working tree に残る）ため、
  この情報で再開可能。
- 除外パターン (.claude/, .omc/, .claude.old/, .understand-anything/, *.local, *.lnk 等)
  以外に staged される実ファイルがゼロの場合もアボート扱い。
- **gitignored 成果物のみ (P-003/step-3, Issue #109)**: staged がゼロで、変更成果物が
  .omc/, target/, dist/, build/, node_modules/, coverage/, *.log, *.tmp 等
  gitignore 対象パスにしか存在しない場合もアボート扱い ('ABORTED' を明記)。
  成果物を gitignore 対象でなく (又は .gitignore 更新 + git add -f で) tracked して
  再実行すること。
- **例外 (R7 整合)**: snapshot commit が既に存在し（上記 R7 確認で判定済み）、
  追加の staged 差分がゼロなら正常パス。アボートせず手順 4-5 へ進め。

Commit Message (gate 合意 — verbatim 使用、scope 編集のみ可):
${consensus.commit_message}

Body (verbatim — 削除禁止):
${consensus.commit_body || ''}

**メッセージ verbatim 保持 (Issue #109)**: 上記の gate 合意 conventional-commit メッセージを
そのまま verbatim で使用せよ。許される編集は scope 部分のみ。
- type の書き換え禁止 (例: feat → feat(snapshot) への変更は不可)
- body の落とし・省略禁止
- Co-Authored-By: glm 4.7 <noreply@zhipuai.cn> trailer の削除禁止（必ず保持）

Ticket: ${ticket.title} (issue #${ticket.issueNumber || 'none'})
受け入れ基準: ${JSON.stringify(ticket.acceptanceCriteria || [])}

Execute:
1. ブランチ作成: git checkout -b ${snapshotBranch}  (既存の場合は git checkout ${snapshotBranch})
2. Stage only relevant files (.claude/, .understand-anything/, *.local, *.lnk 等は除外)
3. **commit 直前に git diff --cached --name-only で空リリース防止アサーション（上記）。
   問題なければ** Commit: 上記 message を verbatim で（scope 編集のみ可）+ Co-Authored-By: glm 4.7 <noreply@zhipuai.cn>
   (snapshot commit が既にあれば、元 conventional-commit メッセージを保持したまま scope のみ修正する amend、又は --amend --no-edit)
4. Push: git push -u origin HEAD
5. PR作成: gh pr create --title "<commit subject>" --base master --body 以下本文:
   <body>
   Closes #${ticket.issueNumber}
   ## Gate evidence
   6-dimension commit gate: all GO
   ## Acceptance criteria
   ${JSON.stringify(ticket.acceptanceCriteria || [])}
報告: branch名, PR URL, PR番号。**最終行に必ず \`PR_NUMBER=<番号>\` を記載**（後続ステップが抽出する）。`;
// [release-prompt-end]
const releaseResult = precheckAbort
  ? `ABORTED (precheck: ${precheckAbort.reason}) — Release agent skipped (Issue #66 Task 3)`
  : await agent(
      buildReleasePrompt(consensus, ticket, snapshotBranch),
      { label: 'release:branch-push-pr', phase: 'Release', model: 'sonnet' }
    );
log('Release: branch + push + PR 作成');
const releaseAbort = resolveReleaseAbort({ precheck: precheckAbort, releaseResult });

let verdicts;
let releaseDecision = { goCount: 0, missingCount: 3, expectedTotal: 3, allGo: false, releaseBlocked: true };
let allGo = false;
let goCount = 0;
// P-006 (cycle-20): mergeResult は else ブロック内で宣言されていたが最終 return が
// ブロック外から参照 → 正常完走時 (3/3 GO merge 実行後) に限り ReferenceError で
// workflow 死亡 (PR #70 マージは実施済みなのに status が返らない)。
// hoist 宣言で修正。以降の Phase 6-7 変数は else ブロック内スコープのまま。
let mergeResult = null;

if (releaseAbort.aborted) {
  // Issue #66 Task 3: skip Phase 6-7 (Release Review / Merge) — nothing was
  // pushed/PR'd, so there is nothing to review or merge. Fall through to
  // Phase 8 Self-Improve (defect-class feedback must still be collected).
  log(`Release ABORTED (${releaseAbort.reason}): skipping Phase 6-7 (Release Review / Merge) → Self-Improve (Issue #66)`);
} else {
// ── Phase 6: Release Review (3レビュアー GO/NO-GO 並列) ──
phase('Release Review');
const REVIEW_SCHEMA = {
  type: 'object',
  required: ['verdict', 'rationale'],
  properties: {
    verdict: { type: 'string', enum: ['GO', 'NO-GO'] },
    rationale: { type: 'string' },
  },
};
const reviewLens = [
  '要件充足: ticket の受け入れ基準が全て PR(コミット内容)で満たされているか。未達があれば NO-GO。',
  '品質証拠: テスト/gate 証拠が PR 本文に提示され再現可能か。コミットが 6次元 gate GO を経ているか。',
  '副作用・スコープ: 既存機能への回帰、スコープ逸脱、Closes #N の対象違い(不適切な issue close)がないか。',
];
// [team-verdict-begin]
// Pure verdict logic shared by the TeamCreate path and the parallel() fallback path.
// Extracted as a marker-delimited block and unit-tested by
// .claude/workflows/tests/team-verdict.test.mjs (TDD, Issue #63 Task 3).
//
// Semantics (INVARIANT — must not change): merge iff exactly 3/3 GO.
// A missing/failed review NEVER counts as GO and blocks the release.
//
// Strict structured token: a teammate verdict message must contain exactly one
// `VERDICT: GO|NO-GO` token (case-insensitive, VERDICT:= prefix required).
// Ambiguous or prose-only messages are MISSING → release blocked. This mirrors
// the StructuredOutput discipline of the fallback path: plain prose is not a
// verdict channel (CTO approval condition: no regression to text-parse leniency).
const TEAM_VERDICT_RE = /\bVERDICT\s*[:=]\s*(NO-GO|NOGO|GO)\b/gi;
const ANY_NOGO_RE = /\b(NO-GO|NOGO)\b/i;
const parseTeamVerdict = (text) => {
  if (typeof text !== 'string' || text.length === 0) return null;
  const matches = [...text.matchAll(TEAM_VERDICT_RE)].map((m) => m[1].toUpperCase());
  if (matches.length !== 1) return null; // zero or conflicting tokens → MISSING
  const verdict = matches[0] === 'NOGO' ? 'NO-GO' : matches[0];
  // Safety: a structured GO alongside any stray NO-GO mention is ambiguous → MISSING (never a false GO).
  if (verdict === 'GO' && ANY_NOGO_RE.test(text.replace(TEAM_VERDICT_RE, ''))) return null;
  return { verdict };
};
const mergeReleaseVerdicts = (verdicts, expectedTotal = 3) => {
  const list = Array.isArray(verdicts) ? verdicts : [];
  const goCount = list.filter((v) => v && typeof v === 'object' && v.verdict === 'GO').length;
  const missingCount = list.filter((v) => !(v && typeof v === 'object' && (v.verdict === 'GO' || v.verdict === 'NO-GO'))).length;
  const allGo = list.length === expectedTotal && goCount === expectedTotal && missingCount === 0;
  return { goCount, missingCount, expectedTotal, allGo, releaseBlocked: !allGo };
};
// [team-verdict-end]

// TeamCreate-based Release Review (Issue #63 Task 3).
// - reviewLens 3視点を teammate に割り当て、verdict を受信して 3/3 GO 判定する。
//   verdict は TaskUpdate metadata (structured) を正経路とし、SendMessage 受信文は
//   parseTeamVerdict (strict VERDICT: token) でのみ補助パースする。
// - ハーネスが TeamCreate/SendMessage/TaskCreate/TaskList/TaskUpdate を公開していない
//   場合（Task 1 probe 未実施のため未保証）は従来の parallel() + StructuredOutput
//   経路へ fall-back する。両経路とも mergeReleaseVerdicts で同一 semantics を強制。
const runReleaseReviewViaTeam = async () => {
  const has =
    typeof TeamCreate === 'function' &&
    typeof SendMessage === 'function' &&
    typeof TaskCreate === 'function' &&
    typeof TaskList === 'function' &&
    typeof TaskUpdate === 'function';
  if (!has) {
    log('Release Review (team): team primitives unavailable in this harness — falling back to parallel() path');
    return null;
  }
  const teamName = `release-review-${runId}`; // Date API 禁制のため runId を使用
  try {
    await TeamCreate({ team_name: teamName, description: `Release Review for issue #${ticket.issueNumber || 'none'}` });
  } catch (e) {
    log(`Release Review (team): TeamCreate failed (${e}) — falling back to parallel() path`);
    return null;
  }
  try {
    const reviewerPrompt = (lens, i) =>
      `RELEASE REVIEWER teammate ${i + 1}/3。視点: ${lens}
Run: runTimestamp=${runTimestamp} (runId=${runId}) — evidence 照合は .omc/logs/${runId}/ 配下の JSON を参照 (Issue #97 UC-2)。
${RUN_TS_CHECK}
PR 情報: ${releaseResult}
Ticket: ${ticket.title} (issue #${ticket.issueNumber || 'none'})
受け入れ基準: ${JSON.stringify(ticket.acceptanceCriteria || [])}
判定: GO(マージ可) / NO-GO(要再作業)。根拠1-2行。

【CRITICAL】あなたの verdict は TaskUpdate metadata (structured) でのみ伝達される。
レビュー完了時、自分の task を TaskUpdate で completed にし metadata に
{ "verdict": "GO"|"NO-GO", "rationale": "..." } を設定すること。
加えて team-lead 宛 SendMessage で「VERDICT: GO」または「VERDICT: NO-GO」の
1トークンを必ず送ること（監査ログ）。prose-only で完了した場合、review は
MISSING とみなされ release 全体が block される。`;
    // Spawn 3 reviewers (one per lens) as teammates.
    const spawnResult = await agent(
      `TEAM SPAWNER。team "${teamName}" に3名の reviewer teammate を追加せよ。
各 teammate へ以下のプロンプトを割り当てること（Agent tool, team_name: "${teamName}"）:
${reviewerLensSpawnerPrompt(reviewerPrompt)}
報告: 追加した teammate 名のリスト。`,
      { phase: 'Release Review', label: 'release-review:team-spawn', model: 'sonnet' }
    );
    // Poll TaskList until all 3 verdict tasks are completed (structured metadata) or timeout.
    // Date API 禁制のため反復回数ベースの cap (15s interval × 80 = 20 min 相当)。
    const MAX_POLLS = 80;
    const collected = new Array(3).fill(null);
    for (let poll = 0; poll < MAX_POLLS; poll++) {
      const tasks = await TaskList();
      for (const t of (tasks || [])) {
        if (!t || !t.owner) continue;
        const idx = reviewLens.findIndex(
          (_, i) => (t.metadata && (t.metadata.lensIndex === i)) || (t.subject || '').includes(`release-review-${i + 1}`)
        );
        if (idx < 0 || collected[idx] != null) continue;
        if (t.status === 'completed' && t.metadata && (t.metadata.verdict === 'GO' || t.metadata.verdict === 'NO-GO')) {
          collected[idx] = { verdict: t.metadata.verdict, rationale: String(t.metadata.rationale || '') };
        }
      }
      if (collected.every((v) => v != null)) break;
      await new Promise((r) => setTimeout(r, 15000));
    }
    const merged = mergeReleaseVerdicts(collected, 3);
    log(`Release Review (team ${teamName}): ${merged.goCount}/3 GO${merged.missingCount > 0 ? `, ${merged.missingCount} MISSING (block)` : ''} [spawn: ${String(spawnResult).slice(0, 200)}]`);
    return merged;
  } finally {
    // TeamDelete requires team_name — argument-less call silently deletes nothing (Issue #63 Task 3).
    try { await (typeof TeamDelete === 'function' ? TeamDelete({ team_name: teamName }) : null); } catch (_e) { /* best effort */ }
  }
};
// helper: build the spawner prompt body (kept outside runReleaseReviewViaTeam for clarity)
function reviewerLensSpawnerPrompt(makePrompt) {
  return reviewLens.map((lens, i) => `--- teammate reviewer-${i + 1} へのプロンプト ---\n${makePrompt(lens, i)}`).join('\n');
}

let releaseReviewMerged = null;
try {
  releaseReviewMerged = await runReleaseReviewViaTeam();
} catch (e) {
  log(`Release Review (team): unexpected failure (${e}) — falling back to parallel() path`);
  releaseReviewMerged = null;
}

verdicts = [];
if (releaseReviewMerged) {
  // Team path completed: use its merged result directly (same 3/3 GO semantics).
  verdicts = new Array(releaseReviewMerged.goCount).fill({ verdict: 'GO' });
} else {
  // Fallback path: original parallel() + StructuredOutput (R6-hardened route, unchanged).
  verdicts = (await parallel(
    reviewLens.map((lens, i) => () =>
      agent(
        `RELEASE REVIEWER ${i + 1}/3。視点: ${lens}
Run: runTimestamp=${runTimestamp} (runId=${runId}) — evidence 照合は .omc/logs/${runId}/ 配下の JSON を参照 (Issue #97 UC-2)。
${RUN_TS_CHECK}
PR 情報: ${releaseResult}
Ticket: ${ticket.title} (issue #${ticket.issueNumber || 'none'})
受け入れ基準: ${JSON.stringify(ticket.acceptanceCriteria || [])}
判定: GO(マージ可) / NO-GO(要再作業)。根拠1-2行。

【CRITICAL】あなたの verdict は StructuredOutput ツール呼び出しでのみ伝達される。
prose-only で終了した場合、review は MISSING とみなされ release 全体が block される。
必ず最後に StructuredOutput({verdict: "GO"|"NO-GO", rationale: "..."}) を呼ぶこと。テキストだけで完了しない。`,
        { schema: REVIEW_SCHEMA, phase: 'Release Review', label: `reviewer:${i + 1}`, model: 'sonnet' }
      )
    )
  )).filter(Boolean);
}
// Unified semantics gate (both paths): 3/3 GO required; missing/NO-GO blocks.
const releaseDecision = mergeReleaseVerdicts(
  releaseReviewMerged ? new Array(3).fill(null).map((_, i) => (i < releaseReviewMerged.goCount ? { verdict: 'GO' } : null)) : [...verdicts, null, null, null].slice(0, 3),
  3
);
goCount = releaseDecision.goCount;
log(`Release Review: ${goCount}/3 GO`);
allGo = releaseDecision.allGo;

// ── Phase 7: Merge & Close ──
phase('Merge & Close');
// mergeResult は hoist 済み (P-006)。ここでは再宣言しない。
if (allGo) {
  mergeResult = await agent(
    `RELEASE MANAGER (Step 3: merge + close)。3/3 GO 確認済み。
PR 情報(最終行 PR_NUMBER=<n> を抽出): ${releaseResult}
Execute:
1. PR番号を上記から抽出
2. gh pr merge <n> --squash --delete-branch
3. issue close 確認: PR 本文 Closes #${ticket.issueNumber} で自動 close されなければ gh issue close ${ticket.issueNumber} --comment "3レビュアーGO・squash merge完了(feature-pipeline)"
4. pipeline run counter: .claude/.pipeline-run-counter を読み+1書き戻し(5の倍数なら RETROSPECTIVE DUE 表示)
報告: merge SHA, ブランチ削除, issue close 状態。`,
    { label: 'release:merge-close', phase: 'Merge & Close', model: 'sonnet' }
  );
  log('Merge & Close: merged + issue closed');
} else {
  log(`Merge skipped: review NO-GO あり (${goCount}/3 GO)`);
}
} // end if (!releaseAbort.aborted) — Issue #66 Task 3: Phase 6-7 skip on abort

// ── Phase 8: Self-Improve（局所最適化を避け、すべての問題の構造的根源を改善）──
phase('Self-Improve');
const selfImprove = await agent(
  `自己改善レビュアー。feature-pipeline の**局所最適化を避け**、すべての問題の**構造的根源**を改善する。

Read .claude/org-feedback.md の全エントリ。
1. パターン集計: 複数エントリにまたがる繰り返し問題を、症状(allow濫用/引数多/scope曖昧/operator-gated 等)でなく**構造カテゴリ**(ゲート設計/スコープ境界/trait 設計/責務分割/外部依存の扱い)で分類。
2. 構造的根源抽出: 各カテゴリの症状を生む feature-pipeline(または anaden-helper)の設計上の根源。
3. **局所最適化チェック**: 改善案が「単一症状の修正(allow禁止ルール追加、引数を構造体に詰める等)」なら**却下**。「複数症状を生む根源の設計変更(Capture/Input trait 統合、per-scope gate、operator-gated タイプ分類、human-in-loop フェーズ化等)」のみ残す。
4. 構造的改善案をレポート(feature-pipeline.js のフェーズ/責務再設計、または anaden-helper のアーキテクチャ)。**適用は人間承認**(自動書き換えしない)。

出力: パターン集計(構造カテゴリ別) / 根源 / **局所最適化を回避した**構造改善案(適用承認待ち)。`,
  { label: 'self-improve', phase: 'Self-Improve', model: 'opus' }
);
log('Self-Improve: 構造改善案を生成');

// Issue #66 Task 3: return status enum — 'released' / 'review-rejected' に
// 'empty-release-aborted' を追加。アボート時は Phase 6-7 をスキップしたままここへ到達。
const finalStatus = releaseAbort.aborted ? RELEASE_ABORT_STATUS : (allGo ? 'released' : 'review-rejected');
void finalStatus;
return {
  status: releaseAbort.aborted ? RELEASE_ABORT_STATUS : (allGo ? 'released' : 'review-rejected'),
  ticket, estimate, approval, implResults,
  gate: { reviews: gateReviews, consensus },
  releaseResult,
  releaseAbort: releaseAbort.aborted ? {
    reason: releaseAbort.reason,
    // 実装は失われたわけではない: R7 snapshot が無かった場合のみアボートするため、
    // working tree / 既存 branch はそのまま残る（再開可能）。
    snapshotBranch,
    resumableInfo: {
      note: 'tracked 変更ゼロ(または除外パターンのみ)のため空リリースをアボートした。Phase 6-7 (Release Review / Merge) はスキップ済み。除外対象外の変更を加えるか、意図的に除外パターンをリリースする場合は手動で release-pipeline を実行すること。',
      snapshotBranch,
      excludedFiles: (precheckAbort && precheckAbort.excluded) || [],
      untrackedCount: (precheckAbort && precheckAbort.untrackedCount) || 0,
    },
  } : null,
  releaseReview: { verdicts, goCount },
  mergeResult,
  selfImprove,
};
