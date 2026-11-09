
export const meta = {
  name: 'ai-coding-loop-formal',
  description: 'mizchi 流 AI coding loop: 視点探索 → umbrella backlog → 決定的 metrics で回す TDD 実装ループ → 危険変更の人間エスカレーション',
  phases: [
    { title: 'Explore', detail: '視点ごとの探索的改善エージェント (読み取り専用)' },
    { title: 'Backlog', detail: '重複統合と優先順位付け (umbrella 化)' },
    { title: 'Implement', detail: 'TDD 実装 — 1 アイテム / 1 コミット、green のみ handoff' },
    { title: 'Evaluate', detail: '評価指標を自己申告でなく再実行して検証' },
    { title: 'Formalize', detail: '不変条件をテストオラクル (pin テスト) 化 — args.formalize 時のみ' },
    { title: 'Escalate', detail: '変更の危険度分類。safe は機械、dangerous は人間へ' },
  ],
}

const FINDINGS_SCHEMA = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string' },
          detail: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'major', 'minor', 'info'] },
          files: { type: 'array', items: { type: 'string' } },
        },
        required: ['title', 'detail', 'severity'],
      },
    },
    summary: { type: 'string' },
  },
  required: ['findings', 'summary'],
}

const BACKLOG_SCHEMA = {
  type: 'object',
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          title: { type: 'string' },
          plan: { type: 'string' },
          files: { type: 'array', items: { type: 'string' } },
          source_findings: { type: 'array', items: { type: 'string' } },
        },
        required: ['id', 'title', 'plan'],
      },
    },
  },
  required: ['items'],
}

const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['done', 'blocked'] },
    commit: { type: 'string', description: '成果物コミットの SHA (green 時のみ)' },
    summary: { type: 'string' },
    blocked_reason: { type: 'string', enum: ['context', 'permission', 'capability'] },
  },
  required: ['status', 'summary'],
}

const EVAL_SCHEMA = {
  type: 'object',
  properties: {
    green: { type: 'boolean' },
    commands: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          cmd: { type: 'string' },
          exit_code: { type: 'number' },
          summary: { type: 'string', description: '生出力の要約行 (自己申告値ではなく貼った実行行)' },
        },
        required: ['cmd', 'exit_code'],
      },
    },
    notes: { type: 'string' },
  },
  required: ['green', 'commands'],
}

const RISK_SCHEMA = {
  type: 'object',
  properties: {
    risk: { type: 'string', enum: ['safe', 'dangerous'] },
    reasons: { type: 'array', items: { type: 'string' } },
    diff_summary: { type: 'string' },
    suggested_action: { type: 'string' },
  },
  required: ['risk', 'reasons', 'suggested_action'],
}

const goal = args?.goal ?? 'バックログを評価指標に従って潰す'
const constraints = args?.constraints ?? ''
const maxIterations = args?.maxIterations ?? 5
const perspectives = args?.perspectives ?? [
  'SRE: テレメトリ・エラー経路・リカバリ挙動の穴',
  '性能: ホットパスの不要な割り当て・O(n^2)・キャッシュ機会',
  'セキュリティ: 攻撃者視点のチェックリスト (localhost に閉じた範囲で検討のみ)',
  '品質: 循環依存・コード重複・命名と保守性',
]
const metrics = args?.metrics ?? [
  'cargo nextest run --workspace',
  'cargo clippy --all-targets -- -D warnings',
  'cargo fmt --all --check',
  'ast-grep scan',
]

const METRICS_BLOCK = metrics.map((m) => `- ${m}`).join('\n')

phase('Explore')
log(`goal: ${goal}`)
const found = await parallel(
  perspectives.map((p, i) => () =>
    agent(
      `## コンテキスト\n- repo: このリポジトリ (Rust workspace + 宣言的パイプライン)\n- goal: ${goal}\n\n## タスク\n「${p}」の視点でコードベースを調査し、改善候補を findings として返す。\n読み取りのみ。ファイル変更・コミットはしない。\n各 finding には根拠 (ファイルパスと行の参照) を含める。推測には「未検証」と明記。`,
      { label: `explore:${i + 1}`, phase: 'Explore', schema: FINDINGS_SCHEMA },
    ),
  ),
)
const findings = found.flatMap((r, i) =>
  r ? r.findings.map((f) => ({ ...f, perspective: perspectives[i] })) : [],
)
log(`findings: ${findings.length} 件 (${perspectives.length} 視点)`)

phase('Backlog')
const backlog = await agent(
  `## タスク\n次の findings を重複統合し、優先順位付きの実装 backlog にする。\n重大度・修正の依存関係・まとめて直せるもの (同一ファイル等) を考慮。\n各 item の plan は TDD で実装できる具体性で (RED テスト→GREEN→リファクタ)。\n\n## findings\n${JSON.stringify(findings, null, 2)}`,
  { phase: 'Backlog', schema: BACKLOG_SCHEMA },
)
const items = (backlog?.items ?? []).slice(0, maxIterations)
log(`backlog: ${backlog?.items?.length ?? 0} 件中 ${items.length} 件をこの run で処理 (maxIterations=${maxIterations})`)

if (args?.createIssues && items.length) {
  await agent(
    `gh issue create で umbrella issue を 1 件作成し、下記 backlog をチェックリスト化して本文に載せる。\nラベル: umbrella。goal: ${goal}\n\n${JSON.stringify(items, null, 2)}`,
    { label: 'umbrella-issue', phase: 'Backlog' },
  )
}

const results = []
for (const item of items) {
  phase('Implement')
  const impl = await agent(
    `## コンテキスト\n- goal: ${goal}\n- item: ${JSON.stringify(item)}${constraints ? `\n- 運用上の制約: ${constraints}` : ''}\n\n## タスク\nTDD で実装してコミットすること。\n1. RED: 振る舞いを pin するテストを先に書き、失敗を確認\n2. GREEN: 最小実装\n3. refactor\n4. 品質チェック (すべて green になってからコミット — green でないものはコミットしない):\n${METRICS_BLOCK}\n5. 1 コミット = 1 論理的変更。コミットメッセージは conventional commits で、末尾に\n   Co-Authored-By: glm 4.7 <noreply@zhipuai.cn> を付ける。\n   git add は対象ファイル明示 (git add -A で無関係な未追跡ファイルを巻き込まない)。\n\n## 再開時の扱い (重要)\n- この item の未コミット編集が作業ツリーに既にある場合は、破棄せずそれを評価して続行せよ。\n- git log にこの item の成果物が既にコミットされている場合は再実装しないこと。\n  品質チェックを再実行して green を確認し、既存コミット SHA を報告して done で返れ。\n- 実装は最小で green になる単位に抑える。スコープを広げず、残作業は summary に\n  follow-up として記録する。\n- 最後には必ず構造化出力 (status/summary) を返して終了すること。文章でのみ終了しない。\n\n実装不能な場合は status=blocked とし、blocked_reason を次から選ぶ:\n- context: 指示や守るべきテストが足りない\n- permission: 権限・環境が足りない\n- capability: 現行モデルには困難`,
    { label: `impl:${item.id}`, phase: 'Implement', schema: IMPL_SCHEMA },
  )
  if (!impl || impl.status !== 'done') {
    results.push({ item: item.id, ...impl, eval: null })
    log(`${item.id}: blocked (${impl?.blocked_reason ?? 'agent 失敗'})`)
    continue
  }

  phase('Evaluate')
  const evaluation = await agent(
    `## タスク\n作業ツリーと直近コミット (${impl.commit ?? 'HEAD'}) に対して、次の評価指標を自分で実行し、\nexit code と要約行を実測で返すこと (実装者に聞かない・推測しない):\n${METRICS_BLOCK}\nすべて exit 0 なら green=true。\n失敗時は失敗したコマンドと最初のエラー行を summary に貼る。`,
    { label: `eval:${item.id}`, phase: 'Evaluate', schema: EVAL_SCHEMA },
  )

  if (evaluation?.green) {
    results.push({ item: item.id, ...impl, eval: evaluation, verified: true })
    log(`${item.id}: green (commit ${impl.commit ?? '?'})`)
  } else {
    const retry = await agent(
      `## コンテキスト\n- item: ${JSON.stringify(item)}\n- 前回コミット: ${impl.commit}\n- 評価指標の失敗: ${JSON.stringify(evaluation?.commands ?? [])}\n\n## タスク\n失敗を修正し、評価指標を全 green にして修正コミットを作る。green にできない場合は status=blocked (reason: capability)。`,
      { label: `fix:${item.id}`, phase: 'Implement', schema: IMPL_SCHEMA },
    )
    const evaluation2 = retry?.status === 'done'
      ? await agent(
          `コミット ${retry.commit} 以降に対して評価指標を再実行し exit code と要約を実測で返す:\n${METRICS_BLOCK}`,
          { label: `eval2:${item.id}`, phase: 'Evaluate', schema: EVAL_SCHEMA },
        )
      : null
    const green2 = evaluation2?.green === true
    results.push({ item: item.id, ...(retry ?? impl), eval: evaluation2 ?? evaluation, verified: green2 })
    log(`${item.id}: ${green2 ? 'green after retry' : 'unverified/blocked after retry'}`)
  }
}

let formal = null
if (args?.formalize) {
  phase('Formalize')
  formal = await agent(
    `## タスク\nこの run で触れた領域 (goal: ${goal}) の不変条件を特定し、テストオラクルとして pin テストを追加せよ。\n- 「実装が満たすべき性質」を既存コードから形式化し、それに従っているかを機械検証するテストを書く\n- 反例になりうる入力があれば、それを回帰テストに落とす\n- 追加テストは ${metrics[0]} で green を確認してコミット\n例 (この repo の先行例): crates/anaden-vision/tests/geometry_drift_regression.rs — 座標系契約の pin。\n\n## 再開時の扱い (重要)\n- 前身の agent が途中まで進めている場合がある: タスクリストに未完了タスクがあれば引き継ぎ、\n  作業ツリーの未コミット編集は破棄せず評価して続行せよ。\n- git add は対象ファイル明示。コミットメッセージ末尾に Co-Authored-By: glm 4.7 <noreply@zhipuai.cn>。\n- スコープは最小の green コミット単位に抑え、残作業は summary に follow-up として残す。\n- 最後には必ず構造化出力 (status/summary) を返して終了すること。`,
    { label: 'formalize', phase: 'Formalize', schema: IMPL_SCHEMA },
  )
}

phase('Escalate')
const risk = await agent(
  `## タスク\nこの run の変更 (git log/diff。上記コミット群) を要約し、リスクを分類せよ。\ndangerous 判定基準 (例): DB スキーマ変更、インフラ/CI 変更、workspace 依存追加、\nシークレット/認証まわり、大面积の公開 API 破壊。それ以外は safe。\nsuggested_action には次の一歩 (PR 作成可 / 人間レビュー必須) を書く。`,
  { phase: 'Escalate', schema: RISK_SCHEMA },
)

let prUrl = null
if (args?.createPr && risk?.risk === 'safe') {
  prUrl = await agent(
    `現在のブランチから gh pr create で PR を作成 (base: master、conventional commits 形式のタイトル)。\n本文には goal・変更・評価指標の実測 (${METRICS_BLOCK}) を載せる。`,
    { label: 'create-pr', phase: 'Escalate' },
  )
}

return {
  goal,
  findings_count: findings.length,
  backlog_total: backlog?.items?.length ?? 0,
  results,
  formalized: formal?.summary ?? null,
  risk,
  pr_url: prUrl,
  escalated: risk?.risk === 'dangerous' ? '人間レビュー必須 (危険変更と判定)' : null,
}
