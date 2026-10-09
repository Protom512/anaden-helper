# Goal execution notes

Canonical tracker: canonical_feature_stories.csv (markdown 版は凡例のみ)。

## Done (ai-coding-loop, 2026-10-10)

- loop 01-05 (前回): StepOutcome enum 構造化 / recovery budget / report channel /
  studio max-iters outcome / template cache (b6c15cb, f8f131b, 9de769d,
  ce26a81, 9bee08c)。
- loop 06 (f7394a3): DEF-006 — studio 開始タスク解決の stem→TaskDef 名修正 +
  manifest 優先 (US-STU-08/09)。nav_to_field_pc に pipeline.toml 整備。
- loop 07 (fd13384): DEF-004 — `anaden run` が未指定時 pipeline.toml の [[goal]]
  を既定ゴールとして採用 (US-GOAL-01)。複数 goal は Any 合成。
- loop 08 (4bb74d3): DEF-005/DEF-007 — launch.toml の --target 誤記、main.rs の
  「PipelineManifest 削除済み」誤主張コメント、README フラグ表の解消
  (US-STU-06/US-DOC-01)。
- loop 09 (ae6dfb6): routine サブコマンドに `--report-file` を追加
  (03-report-channel follow-up 解消)。engine の RoutineSummary を Serialize 化 +
  outcome_label()、studio は PendingReportKind で run/routine のレポート種別を
  明示切替し routine 実行の成否・発火数を履歴/ステータスバーへ反映。
  ゲート: nextest 1014 passed, 10 skipped / clippy -D warnings / fmt /
  ast-grep すべて green。
- loop 09 (検証パス): CSV 67 Unverified を一括検証 — live CLI (--help/--version/
  dry-run) + ストーリー別テストフィルタ実行 (run_exit_code 13 / recovery 20 /
  parse_goal_flag 8 / manifest 42 / goal 135 / studio 452 ほか) + コード突合で
  63 Pass / 4 Blocked (US-PIP-01/02/03/05 = 実ゲーム必要) に確定
  (72 stories: 68 Pass / 4 Blocked / 0 Unverified / 0 Fail)。

## Open (次回ループ)

1. Blocked 4 ストーリーの実機検証 (US-PIP-01/02/03/05): 実ゲーム環境で
   nav_to_field_pc コールドスタート・field_loop_pc probe・login・
   title_live_regression を実行して Pass 化する。
2. US-GOAL-01 のライブゲーム確認: manifest goal でライブ run が停止する実機
   検証は未実施 (コード+テスト証拠のみ)。実機環境で login シナリオ実行。
3. routine --report-file の実 GUI 確認: studio から routine を spawn し
   レポート読込が履歴/ステータスバーへ反映されることを実機で確認
   (unit テスト済み・GUI 操作は未実施)。run と違い routine は既定永続化
   (report.json + run.log) が無い — 必要なら follow-up。
4. F 系 backlog の未消化分 (F5, F8-F10, F12, F14-F16, F18-F33 の一部) は
   探索テーブルが永続化されていないため、再探索が必要な場合は CSV の
   Blocked/notes 行から再導出すること。

## 前ループからの継続エスカレーション (対処禁止 — 人間判断待ち)

1. 複数 `[[goal]]` の Any 合成セマンティクス (loop 07 で暫定採用)。
2. manifest ゴール既定採用の実機検証 (Open 2 と同一)。
3. 未追跡ディレクトリ `.grok/`, `.hermes/`, `personas/`, `skills/`,
   `docs/explainers/` のコミット/gitignore 判断 — これらはそのまま放置する。
