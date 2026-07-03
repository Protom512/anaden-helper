# Test Plan — anaden-helper

[canonical_feature_stories.md](canonical_feature_stories.md) の各 US に対応するテストケース。Tier 順に実行する。

**最終実行**: 2026-06-24 | **Unit**: 208/208 PASS | **Offline**: field_loop_pc PASS

## Tier 実行順

1. **Unit** — `cargo test --workspace`
2. **Offline** — `cargo run --release -p anaden-cli --bin anaden-tool -- run-pipeline ...`
3. **PC-Live** — `cargo run --release -p anaden-cli --bin anaden -- run ... --target windows`
4. **Android-Live** — `cargo run --release -p anaden-cli --bin anaden --features capture-scrcpy -- run <serial> ...`

---

## Pipeline / CLI

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-P01-01 | US-P01 | Unit | `pipeline_driver::run_loop_*` (22 tests) | Loop outcomes correct | Pass |
| TC-P01-03 | US-P01 | Android-Live | `anaden run emulator-7554 templates/pipelines/field_loop TapBottomStable --capture scrcpy --input scrcpy --max-iters 1` | 1 cycle Fired or documented NoMatch | Blocked |
| TC-P02-01 | US-P02 | Unit | `main.rs` target=windows branch compiles; Win32 types wired | No ADB serial required | Pass |
| TC-P02-02 | US-P02 | PC-Live | `anaden run templates/pipelines/field_loop_pc TapBottomStablePc --target windows --max-iters 1 --ensure-open false` | Capture+1 iter (game running) | Blocked |
| TC-P03-01 | US-P03 | Unit | `run_loop_reaches_terminal_task` | TerminalTask, 3 fired | Pass |
| TC-P04-01 | US-P04 | Unit | `pipeline::action_*` parse tests | All action types parse | Pass |
| TC-P05-01 | US-P05 | Unit | `verify_success_when_template_disappears`, `verify_fails_when_template_persists` | Fired vs FiredUnverified | Pass |
| TC-P06-01 | US-P06 | Unit | `recovery_hook_fires_after_nomatch_threshold` | Hook ≥1 call | Pass |
| TC-P07-01 | US-P07 | Unit | `app_control::ensure_app_open_*` (5 tokio tests) | AlreadyOpen/Launched/Timeout | Pass |
| TC-P08-01 | US-P08 | Unit | `detect_switches_sse_and_ccoeff` | Algorithm switch changes result | Pass |
| TC-P09-01 | US-P09 | Unit | scrcpy module compiles with feature | feature gate OK | Pass |
| TC-P09-02 | US-P09 | Android-Live | scrcpy capture 1 frame <500ms | E2E capture | Blocked |
| TC-P10-01 | US-P10 | Android-Live | scrcpy-touch 1 tap accepted by game | Screen change | Blocked |

---

## Device I/O

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-D01-01 | US-D01 | Unit | ScreenshotCapture via device tests | PNG decode | Pass |
| TC-D02-01 | US-D02 | Unit | InputExecutor command construction | shell input format | Pass |
| TC-D03-01 | US-D03 | Android-Live | ScrcpyCapture first frame | H.264 decode OK | Blocked |
| TC-D04-01 | US-D04 | Unit | `scrcpy_session.rs` protocol tests (4) | Touch packet encode | Pass |
| TC-D05-01 | US-D05 | Unit | `aspect_ratio_degradation`, `pc_capture_probe_has_measured_1258x708_dimensions` | 1258×708 | Pass |
| TC-D06-01 | US-D06 | Unit | `win32_input.rs` coordinate tests (3) | SendInput coords | Pass |
| TC-D07-01 | US-D07 | Unit | `win32_launch` spawn logic | Launcher path constants | Pass |
| TC-D08-01 | US-D08 | Unit | `app_control.rs` (16 tests) | dumpsys parse | Pass |
| TC-D09-01 | US-D09 | Unit | `display.rs` + studio `black_frame_*` | stayon path | Pass |

---

## Vision

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-V01-01 | US-V01 | Unit | `engine::sse_engine_finds_best_match` | Match found | Pass |
| TC-V02-01 | US-V02 | Unit | `ccoeff_robust_to_brightness_shift_sse_degrades` | CCOEFF stable | Pass |
| TC-V03-01 | US-V03 | Unit | `scale::*`, `rescale_tap_pixel7a_2400_*` | Roundtrip coords | Pass |
| TC-V04-01 | US-V04 | Unit | `scene_detector::tests::*` (5) | Voting logic | Pass |
| TC-V05-01 | US-V05 | Unit | `template_store::tests::*` | State parse | Pass |
| TC-V06-01 | US-V06 | Unit | `hud_tr_20to9_template_degrades_to_nonmatch_on_pc_16to9_frame` | conf<0.80 on PC | Pass |

---

## Template Pipelines

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-T01-01 | US-T01 | Unit | `load_pipeline(field_loop)` | 2 tasks load | Pass |
| TC-T02-01 | US-T02 | Unit | `pc_field_loop_pc_templates_match_real_capture_above_threshold` | conf≥threshold | Pass |
| TC-T02-02 | US-T02 | Offline | `anaden-tool run-pipeline templates/captures/field_pc_probe.png templates/pipelines/field_loop_pc TapBottomStablePc --algorithm ccoeff` | Match + Tap coords | Pass |
| TC-T02-03 | US-T02 | Offline | 同上 `TapHudTrPc` | Match | Pass |
| TC-T03-01 | US-T03 | Android-Live | `worldmap_loop/TapAncientTab --max-iters 1` | Tab UI change | Blocked |
| TC-T04-01 | US-T04 | Android-Live | `nav_to_field` chain on device | Popup dismiss | Blocked |
| TC-T05-01 | US-T05 | Unit | `pc_nav_to_field_points_at_pc_field_hud_template` | FieldHudTopPc loads | Pass |
| TC-T06-01 | US-T06 | — | — | Experimental; not in scope | N/A |

---

## anaden-studio

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-S01-01 | US-S01 | Unit | `scoring::tests::*`, `proposals::tests::*` | Discriminability | Pass |
| TC-S02-01 | US-S02 | Unit | `batch::tests::*` (3) | Confusion matrix | Pass |
| TC-S03-01 | US-S03 | Unit | `source::tests::*` (7) | Live capture safety | Pass |
| TC-S04-01 | US-S04 | Unit | `library::save_then_load_roundtrip` | PNG+TOML roundtrip | Pass |

---

## anaden-tool

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-C01-01 | US-C01 | Manual | `anaden-tool extract` + `match` on field_pc_probe | PNG written; confidence printed | Pass |
| TC-C02-01 | US-C02 | Unit | `collector::tests::*` (8) | Grouping stable | Pass |
| TC-C03-01 | US-C03 | Offline | `run-pipeline` TapBottomStablePc + TapHudTrPc | Both match | Pass |
| TC-C04-01 | US-C04 | Android-Live | `anaden-tool record emulator-7554` | PNG sequence | Blocked |

---

## Legacy

| TC-ID | US-ID | Tier | Command / Test | Expected | Last Result |
| :--- | :---: | :--- | :--- | :--- | :---: |
| TC-L01-01 | US-L01 | Unit | `state_machine.rs`, `orchestrator` tests | FSM transitions | Pass |

---

## ループD 回帰チェックリスト

- [x] `cargo test --workspace` — 208 tests green
- [x] Offline `run-pipeline` field_loop_pc 両タスク
- [x] DEF-001〜003 修正後再検証
- [ ] PC-Live（AnotherEden.exe 起動時）
- [ ] Android-Live scrcpy 完全ループ（TASKS.md 保留分）
