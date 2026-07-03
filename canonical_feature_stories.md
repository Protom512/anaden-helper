# Canonical Feature Story Spreadsheet

Another Eden 自動操作ツール **anaden-helper** の単一正準ソース。コードから導出した期待動作・検証ステータス・欠陥 ID を追跡する。

関連: [test_plan.md](test_plan.md) | [defects_log.md](defects_log.md)

**最終更新**: 2026-06-24 | **ユニットテスト**: 208 green

## ステータス凡例

| Status | 意味 |
| :---: | --- |
| Pass | 該当 Tier の TC すべて成功 |
| Blocked | 環境依存（実機/PC ゲーム未起動等）で未実施 |
| N/A | 未実装・後方互換のみ |

---

## Pipeline / CLI (`anaden run`)

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-P01 | As an operator, I want to run a declarative pipeline on Android so that capture→normalize→tick→rescale→execute loops automate field actions. | `crates/anaden-cli/src/main.rs`, `crates/anaden-engine/src/pipeline_driver.rs` | `anaden run <serial> <dir> <task>` が MaxIterations/TerminalTask で終了。panic しない。 | Unit, Android-Live | TC-P01-01, TC-P01-03 | Blocked | — | Android-Live: 実機 Another Eden + scrcpy feature build 要 |
| US-P02 | As a PC player, I want `--target windows` so that Win32 capture/input run without ADB serial. | `main.rs::run_with_windows` | `--target windows` で serial 不要。Win32Capture + Win32InputExecutor 使用。 | Unit, PC-Live | TC-P02-01, TC-P02-02 | Pass | DEF-002 | PC-Live: AnotherEden 未起動時は Device I/O failed（修正済） |
| US-P03 | As a pipeline author, I want TOML `next` chains so that tasks transition after each fired step. | `crates/anaden-vision/src/pipeline.rs`, `pipeline_driver::run_loop_reaches_terminal_task` | 3 タスク chain が TerminalTask で停止。current が next へ更新。 | Unit | TC-P03-01 | Pass | — | |
| US-P04 | As a pipeline author, I want action types click_self/click_rect/swipe/do_nothing/stop so that diverse inputs are declarable. | `pipeline.rs` Action enum, TOML parse tests | 各 action が parse 可能。Stop は発火なし NoFire。 | Unit | TC-P04-01 | Pass | — | |
| US-P05 | As an operator, I want `--verify-after-fire` (default true) so that false success is detected when template persists. | `pipeline_driver::run_once_verified`, `verify_fails_when_template_persists` | テンプレ残存 → FiredUnverified。消失 → Fired + next 遷移。 | Unit | TC-P05-01 | Pass | — | |
| US-P06 | As an operator, I want NoMatch recovery via `--recover-launch` so that stuck states trigger game relaunch. | `pipeline_driver::recovery_hook_fires_after_nomatch_threshold` | threshold 到達で RecoveryHook 呼出。threshold=0 で無効。 | Unit | TC-P06-01 | Pass | — | |
| US-P07 | As an operator, I want `--ensure-open` so that the game is foregrounded before the loop. | `main.rs`, `app_control.rs` | dumpsys + am start / Win32Launch。AlreadyOpen/Launched/Timeout。 | Unit | TC-P07-01 | Pass | — | |
| US-P08 | As an operator, I want `--algorithm sse\|ccoeff` override so that recognition strategy is selectable. | `main.rs::resolve_algorithm`, `pipeline::detect_switches_sse_and_ccoeff` | 非法値は bail。ccoeff/sse 切替で detect 結果が変化。 | Unit | TC-P08-01 | Pass | — | |
| US-P09 | As an operator, I want `--capture screencap\|scrcpy` on Android so that capture latency is tunable. | `main.rs`, `scrcpy.rs` | screencap 既定。scrcpy は capture-scrcpy feature 必須。 | Unit, Android-Live | TC-P09-01, TC-P09-02 | Blocked | — | scrcpy 実機 E2E 保留（TASKS.md） |
| US-P10 | As an operator, I want `--input scrcpy\|adb` so that anti-cheat tap bypass is available. | `scrcpy_session.rs`, `main.rs` | scrcpy input は video+control 2 ソケット一本化。 | Unit, Android-Live | TC-P10-01 | Blocked | — | |

---

## Device I/O

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-D01 | As the engine, I want adb screencap so that screenshots are captured via exec-out. | `screenshot.rs` | `adb exec-out screencap -p` → PNG DynamicImage。 | Unit | TC-D01-01 | Pass | — | 統合 E2E は Android-Live |
| US-D02 | As the engine, I want adb shell input so that tap/swipe commands reach the device. | `input.rs` | Tap/Swipe InputAction → shell input コマンド。 | Unit | TC-D02-01 | Pass | — | ゲームは adb input 無視（scrcpy 推奨） |
| US-D03 | As the engine, I want scrcpy H.264 capture so that sub-100ms frames are available. | `scrcpy.rs` | feature capture-scrcpy。常駐デコード。 | Unit, Android-Live | TC-D03-01 | Blocked | — | |
| US-D04 | As the engine, I want scrcpy-touch injection so that Another Eden accepts taps. | `scrcpy_session.rs` | TYPE_INJECT_TOUCH_EVENT。セッションプロトコルテスト green。 | Unit, Android-Live | TC-D04-01 | Blocked | — | |
| US-D05 | As the engine, I want Win32 PrintWindow capture so that PC client frames are readable. | `win32_capture.rs` | 1258×708 RAW。aspect_ratio_degradation テスト。 | Unit, Offline | TC-D05-01 | Pass | — | field_pc_probe.png |
| US-D06 | As the engine, I want Win32 SendInput so that PC clicks are injected. | `win32_input.rs` | 座標変換テスト 3 件 green。 | Unit | TC-D06-01 | Pass | — | |
| US-D07 | As the engine, I want Win32 Launcher.exe spawn so that PC game cold-start works. | `win32_launch.rs` | ensure_open → AlreadyOpen/Launched/Timeout。 | Unit | TC-D07-01 | Pass | — | |
| US-D08 | As the engine, I want app foreground detection so that `--ensure-open` is reliable. | `app_control.rs` | dumpsys 解析 16 tests。launch + poll。 | Unit | TC-D08-01 | Pass | — | |
| US-D09 | As the engine, I want screen_off_timeout extension so that black screencap loops are avoided. | `display.rs` | ensure_stay_on()。studio source テスト参照。 | Unit | TC-D09-01 | Pass | — | |

---

## Vision / Recognition

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-V01 | As the engine, I want SSE template matching so that fast normalized matching works on stable UI. | `engine.rs`, `matcher.rs` | 完全一致 conf≈1.0。閾値下は None。 | Unit | TC-V01-01 | Pass | — | |
| US-V02 | As the engine, I want CCOEFF matching so that lighting-invariant recognition works. | `ccoeff.rs` | 照明シフトで SSE 劣化・CCOEFF 維持。 | Unit | TC-V02-01 | Pass | — | |
| US-V03 | As the engine, I want width-1280 normalization + rescale so that 720p-base coordinates map to device pixels. | `scale.rs`, `pipeline_driver::rescale_*` | Pixel7a 2400→(1200,675)。PC 1258 RAW パススルー。 | Unit | TC-V03-01 | Pass | — | |
| US-V04 | As the engine, I want SceneDetector voting so that game state is inferred from multiple templates. | `scene_detector.rs` | min_votes=2。Unknown on non-match。 | Unit | TC-V04-01 | Pass | — | |
| US-V05 | As the engine, I want TemplateStore directory=GameState so that scene templates are organized. | `template_store.rs` | ディレクトリ名→GameState parse。 | Unit | TC-V05-01 | Pass | — | |
| US-V06 | As a template author, I want aspect-ratio isolation so that 20:9 templates are not reused on PC 16:9. | `tests/aspect_ratio_degradation.rs` | hud_tr 20:9 conf~0.99 → PC 0.67 NoMatch。 | Unit | TC-V06-01 | Pass | — | |

---

## Template Pipelines (実フロー)

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-T01 | As a player, I want `field_loop/` (Android) so that stable field UI taps loop on 20:9 device. | `templates/pipelines/field_loop/` | TapBottomStable/TapHudTr TOML load。conf 設計 0.80+。 | Unit, Android-Live | TC-T01-01 | Blocked | — | 20:9 実機キャプチャ要 |
| US-T02 | As a PC player, I want `field_loop_pc/` so that field taps work on 1258×708 RAW space. | `templates/pipelines/field_loop_pc/` | field_pc_probe 上 conf≥0.80。run-pipeline マッチ。 | Unit, Offline, PC-Live | TC-T02-01, TC-T02-02 | Pass | DEF-001 | ROI/テンプレ修正済 |
| US-T03 | As a player, I want `worldmap_loop/TapAncientTab` so that ancient tab selection automates. | `templates/pipelines/worldmap_loop/` | TOML load + ccoeff。E2E 効果=タブ変化。 | Unit, Android-Live | TC-T03-01 | Blocked | — | TASKS.md 推奨 E2E 候補 |
| US-T04 | As a player, I want `nav_to_field/dismiss_daily_popup` so that daily popup closes before field. | `nav_to_field/dismiss_daily_popup.toml` | close_btn conf 1.0 設計。→ FieldHudTop chain。 | Unit, Android-Live | TC-T04-01 | Blocked | — | |
| US-T05 | As a PC player, I want `nav_to_field_pc/` cold-start chain so that title→field navigation works. | `templates/pipelines/nav_to_field_pc/` | FieldHudTopPc 存在。契約テスト load。 | Unit | TC-T05-01 | Pass | — | 実機 title プローブは別 Issue |
| US-T06 | As a player, I want `_title_load/load_game` experimental pipeline so that cold-start from title is attempted. | `templates/pipelines/_title_load/` | 実験的 threshold 0.01。テンプレ品質未完了。 | — | TC-T06-01 | N/A | — | TASKS.md バックログ |

---

## anaden-studio

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-S01 | As a template author, I want ROI drag + discriminability score so that false positives are caught early. | `studio/scoring.rs`, `app.rs` | 黒/白 ROI margin テスト。ccoeff 既定。 | Unit | TC-S01-01 | Pass | — | GUI 手動確認は別途 |
| US-S02 | As a template author, I want batch confusion matrix so that template sets are evaluated holistically. | `studio/batch.rs` | sensitivity/specificity テスト green。 | Unit | TC-S02-01 | Pass | — | |
| US-S03 | As a template author, I want live ADB/Win32 capture in studio so that templates are authored from real frames. | `studio/source.rs` | black frame 検出。bounded channel 1 フレーム。 | Unit | TC-S03-01 | Pass | — | |
| US-S04 | As a template author, I want PNG+sidecar TOML save so that templates integrate with pipeline/scenes. | `studio/library.rs` | save/load roundtrip。multiple states。 | Unit | TC-S04-01 | Pass | — | |

---

## anaden-tool (CLI)

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-C01 | As a developer, I want capture/extract/match/detect commands so that templates are debugged offline. | `template_tool.rs` | capture→PNG。match→confidence 表示。 | Manual | TC-C01-01 | Pass | — | extract/match 実施済 |
| US-C02 | As a developer, I want `explore` so that exploratory template collection runs on device. | `collector.rs` | grouping/similarity 8 tests green。 | Unit, Android-Live | TC-C02-01 | Blocked | — | |
| US-C03 | As a developer, I want `run-pipeline` dry-run so that 1 tick is verified without firing. | `template_tool.rs::run_pipeline_dry` | マッチ時 Tap 座標+next 表示。発火なし。 | Offline | TC-C03-01 | Pass | DEF-003 | field_loop_pc 2 タスク Pass |
| US-C04 | As a developer, I want launch/record commands so that capture sessions are scripted. | `template_tool.rs` | launch am start。record 連続 screencap。 | Manual, Android-Live | TC-C04-01 | Blocked | — | |

---

## Legacy Orchestrator

| US-ID | User Story | Code Evidence | Acceptance Criteria | Test Tier | TC-IDs | Status | Defect IDs | Notes |
| :---: | --- | --- | --- | :--- | :--- | :---: | :--- | --- |
| US-L01 | As a maintainer, I want `anaden legacy` so that SceneDetector loop remains for backward compatibility. | `orchestrator.rs`, `main.rs::Legacy` | SceneDetector 投票。StrategyRegistry 空→Unknown。 | Unit | TC-L01-01 | Pass | — | 新規開発は `run` 推奨 |
| US-L02 | As a player, I want MiniGameStrategy (fishing etc.) so that minigames are automated. | `strategies/registry.rs`, `MiniGameStrategy` trait | 本番戦略未登録。dummy のみ。 | — | — | N/A | — | 将来実装 |

---

## サマリー（2026-06-24）

| Status | 件数 |
| :--- | ---: |
| Pass | 28 |
| Blocked | 11 |
| N/A | 2 |
| **合計 US** | **41** |
