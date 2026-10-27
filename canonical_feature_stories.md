# Canonical Feature Stories

**正準ソースは `canonical_feature_stories.csv` のみ。** 本 Markdown は案内と凡例。機能行・ステータスは CSV を更新すること。

関連: [defects_log.md](defects_log.md)

**最終更新**: 2026-10-05（Issue #188 後の現行 PC/Windows コードから再盤点。旧 Android 行は現行機能ではない）

## ステータス凡例

| Status | 意味 |
| :---: | --- |
| Unverified | コードから物語は書いたが、このゴールのテスト未実施 |
| Pass | 期待動作を現行証拠で確認 |
| Fail | テストまたはコード突合で期待と不一致 |
| Blocked | 実機ゲーム等の環境依存で未実施 |
| N/A | 意図的未実装（グレー表示スタブ等） |

## CSV 列

`us_id,area,user_story,expected_behaviour,code_evidence,verification,status,defect_ids,pre_fix_result,post_fix_result,notes`
