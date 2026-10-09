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

CSV 5 Fail 行は全て Pass 化 (72 stories: 67 Unverified / 5 Pass / 0 Fail)。

## Open (次回ループ)

1. CSV 67 Unverified stories の検証 (test every CSV story; document Fail;
   fix logistics/UX; retest)。多くは実ゲーム (Blocked) かコード突合で判定可能。
2. US-GOAL-01 のライブゲーム確認: manifest goal でライブ run が停止する実機
   検証は未実施 (コード+テスト証拠のみ)。実機環境で login シナリオ実行。
3. routine サブコマンドの --report-file 対応 (03 の follow-up・studio 側未対応)。
4. F 系 backlog の未消化分 (F5, F8-F10, F12, F14-F16, F18-F33 の一部) は
   探索テーブルが永続化されていないため、再探索が必要な場合は CSV の
   Unverified 行から再導出すること。
