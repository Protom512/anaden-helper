# Defects Log — anaden-helper

`canonical_feature_stories.md` のユーザーストーリー検証（ループB）で発見した Logic / UX 欠陥を追跡する。

| Defect ID | Affected US-ID | Test Case ID | Type | Description of Error | Steps to Reproduce | Severity | Status |
| :---: | :---: | :---: | :---: | :--- | :--- | :---: | :---: |
| DEF-001 | US-T02 | TC-T02-02 | Logic | `field_loop_pc` の `TapBottomStablePc` / `TapHudTrPc` が `field_pc_probe.png` 上で threshold 0.80 未満（ccoeff）。`scenes/field_pc` は conf=1.0 でマッチするが、パイプライン用 ROI/テンプレが実キャプチャとずれている。 | 1. `anaden-tool run-pipeline templates/captures/field_pc_probe.png templates/pipelines/field_loop_pc TapBottomStablePc --algorithm ccoeff` | High | Fixed |
| DEF-002 | US-P02 | TC-P02-02 | UX | `--target windows` で AnotherEden 未起動時、Win32 エラーが `AdbError::CommandFailed` 経由で **「ADB command failed」** と表示され原因特定が混乱する。 | 1. AnotherEden.exe を終了 2. `anaden run ... --target windows` | Medium | Verified |
| DEF-003 | US-C03 | TC-C03-01 | UX | `run-pipeline` の非マッチメッセージが「未知タスク名」を列挙するが、実際は閾値下のみ。誤解を招く。 | 1. 存在する task 名で閾値下の probe を指定 | Low | Verified |

## 修正メモ

- **DEF-001**: `field_loop_pc` ROI/テンプレを `field_pc_probe.png` 上の実マッチ領域（`FieldPcTemplate01` / `FieldPcHudTopRight`）に合わせ再オーサリング。契約テスト `pc_field_loop_pc_templates_match_real_capture_above_threshold` を追加。
- **DEF-002**: `AdbError::CommandFailed` の表示を `Device I/O failed` に変更（Win32/ADB 共通）。
- **DEF-003**: 非マッチメッセージから「未知タスク名」を除去。
