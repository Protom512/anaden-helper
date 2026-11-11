# ログイン E2E — 試行ログ (2026-11-11, DEF-009/010/011/012)

1 ユニット = 1 変更 = 1 固定検査。採用/却下を問わず全試行を残す。
検査は「目視で良さそう」ではなく、コマンド出力かテストで判定する。
生ログは `.omc/logs/` (追跡外) にあり、ここには判定と再現コマンドだけを残す。

前提となる実測値 (2026-11-11 20:53 のライブ実行, `live-20261111-def012/`):

| 項目 | 値 |
|---|---|
| cycles | 13 |
| fired | 0 |
| `per_task_matches` | `[]` |
| recovery_attempts | 1 (clamp 成功) |
| 終端 | `goal_timeout` (90s goal / 実測 94.1s) |

---

## U1. 配置 fault を kill+再起動からウィンドウ clamp へ

- 狙い: 配置欠陥 (DEF-009 gate 拒否) からの復帰でゲーム状態を壊さない。
- 試行 1 (却下): 従来どおり `restart_app` — 再起動自体がウィンドウ配置を再現し、
  ロード中の状態を破壊する (DEF-011/012)。実測: 回復のたびに同じ fault に戻る。
- 試行 2 (採用): `clamp_window_into_monitor` / `clamped_top_left` (純関数)。
  検査: `crates/anaden-device/tests/window_clamp_tests.rs` (13 tests) +
  実機 `test-a-clamp.log` — ウィンドウ `(-2200,-2200)` → `Moved { to: (0,0) }`、
  PID 不変・kill 0。**採用**。
- 却下理由の一般化: 「配置欠陥の回復に再起動を使うと、回復が欠陥を再生産する」。

## U2. 回復判断を純関数の表にする

- 狙い: 回復方針を Win32 / 実ゲーム / async driver 抜きで検証可能にする。
- 試行 1 (置換): `escalation_recovery_hook` 内の 3 分岐 match。ライブでは動いたが、
  検証は実機ヒモ付き。**置換**。
- 試行 2 (採用): `RecoveryPlan` + `plan_recovery(cause, attempt, app_alive)` +
  `recovery_cause_for(kind)`。検査: `recovery_policy_tests.rs` の表テスト
  (配置 fault は attempt/alive に依らず ClampWindow、Transient は attempt1=Launch /
  生存=WaitForExisting / 不在=Restart)。**採用**。

## U3. 配置 clamp で boot grace を張らない

- 試行 1 (失敗): 既存の `guard.recovered()` が回復種別を問わず 180s grace を張る。
  実測 `e2e-run.log`: `配置補正完了 → recovery hook succeeded; resetting streak +
  boot grace 180s` の直後から 13 サイクルすべて match 0、`per_task_matches: []`、
  90s goal を grace 内で消費して `goal_timeout`。**失敗**。
- 試行 2 (採用): `RecoveryCause::arms_boot_grace()` は Transient のみ true、
  `RecoveryGuard::rearmed()` は streak だけリセット。
  検査: `execute_error_state_tests::placement_recovery_does_not_arm_a_boot_grace`
  — 3 サイクル中 recovery が 3 回呼ばれる (= grace で盲目化しない)。**採用**。

## U4. 発火失敗時に `current` を戻す

- 診断 (試行 1): `PipelineState::tick` はアクション実行前に `current` を次タスクへ
  進める。`GateRejected` は発火していないのに advance が残るため、以降の全サイクルが
  「到達していない次タスク」を照合し続ける。`per_task_matches: []` はこの症状と一致。
- 試行 2 (採用): execute 失敗時は `set_current(matched_task_name)`、verify capture
  失敗時は `set_current(task_name)`。
  検査: `execute_error_state_tests::execute_error_keeps_the_matched_task_current`
  — 1 回目 `GateRejected` 後も current は `A`、2 回目は `Fired` して `B` へ。**採用**。

## U5. ロードゲームボタンのテンプレート化

- 試行 1 (却下): 既存の `TapToStartPc` 中心 `(915,481)` / `LoadGamePc` 中心
  `(615,273)` を流用 — 実ラベルより約 280px 上で、クリックがボタンに当たらない
  (DEF-011)。**却下**。
- 試行 2 (採用): 追跡下 4K 実機フレームを 1258x708 に正規化し、ラベル中心から 192x52 を
  切り出し (`templates/scenes/title_pc/load_game_button.png`)、
  `roi = [500,480,300,140]` (pipeline task)、`threshold = 0.80`、`action = click_self`。
  検査: `login_load_button_regression.rs` (3 tests)
  - `title_live_1952x1098.png`: confidence ≥ 0.95、中心 x∈[600,690] y∈[520,600]
  - `title_menu_live_20261111_3783x2128.png` (実機 4K): confidence ≥ 0.90、
    中心 x∈[600,690] y∈[510,590]
  - 旧固定 rect `(615,273)` から 100px 以上離れていること (退行の負の記録)
  **採用**。
- 試行 3 (却下): scene def (`scenes/title_pc/load_game_button.toml`) を追加し、ROI を
  title_pc の小テンプレ規約 (≤180px) に収めるためテンプレを 164x52 へ切り詰め →
  `title_live_1952x1098.png` の confidence が **0.816** まで低下 (floor 0.95 未達)。
  規約内 (ROI 180) に収まる針では実ラベルの幅 (192px) を覆えず、覆うと探索窓の余裕が
  2px しか残らない。def は作らず、検出 ROI は pipeline task が持つ。**却下**
  (`pc_title_pc_scene_templates_load_and_validate` は def が無ければ何も要求しない)。


## U6. `nav_to_field_pc/load_game.toml` の固定 rect 修正 — 却下

- 試行 1 (却下/巻き戻し): `LoadGamePc` を `click_self` + 新テンプレートへ変更。
  しかし pinned 構造テスト (`pc_nav_to_field_cold_start_chain_is_walkable_to_field` 等)
  は `LoadGamePc` が `Action::ClickRect` で `title_logo_corner` の template/roi を
  共有していることを要求しており、通すにはテスト改変が必要。
  `assert_tests_untouched.mjs` がテスト改変を拒否するため **巻き戻し**。
- 現状: `nav_to_field_pc` は DEF-011 として未修正のまま。E2E は新設の
  `templates/pipelines/login` チェーンで行う (pinned 契約の外側)。

## U7. `tap_title.toml` の `next` をロードクリックへ張り替え — 却下

- 試行 1 (却下/巻き戻し): `next = ["LoginClickLoadPc"]` に変更。狙いはタップ→ロード→
  フィールドのフルチェーン化。しかし engine の pinned test
  `pipeline_driver::tests::run_once_fires_on_real_title_fixture_through_guarded_preprocessing`
  が `next_current == Some("LoginWaitFieldPc")` を assertion しており、
  nextest で **FAIL** (`left: Some("LoginClickLoadPc")`)。
  期待値改変は `#[cfg(test)]` 領域の編集になり `assert_tests_untouched.mjs` が拒否する。
- 代替: E2E の start を `LoginClickLoadPc` 明示にして、チェーン接続の修正は
  DEF-013 として記録。**巻き戻し**。

## U8. ライブ E2E

- 試行 1 (失敗): `anaden run templates/pipelines/login LoginTapTitlePc`
  → 13 iters / 0 fires / `goal_timeout`。U3・U4 の原因を産んだ。
- 試行 2 (未実行): `LoginClickLoadPc` から開始する E2E。実ゲームへのクリック注入を
  含む不可逆操作のため、ユーザー承認待ち。
- 到着判定 (試行 2 の合否条件): `hud_top` (通貨バー) は街に存在しない (DEF-008) ため、
  `login/watch/field_town_bar.toml` (`LoginFieldTownBarPc`, action=stop) を
  nav の実機検証済み town bar 針への相対参照で追加した。街/フィールド到着で
  `watch stop` → `Stop アクション到達(正常)` → exit 0。
  合否は (a) `report.json` の `fired_count >= 1` かつ `per_task_matches` に
  `LoginClickLoadPc`、(b) 実行後フレームで `LoginClickLoadPc` と
  `LoginTapTitlePc` が NoMatch (タイトルから離脱した)、(c) 可能なら
  `watch stop: LoginFieldTownBarPc` の 3 点で判定する。

---

## チェック結果 (2026-11-11, このツリー)

| コマンド | 結果 |
|---|---|
| `node scripts/quality/complexity_gate.mjs` | violations: 0 |
| `node scripts/quality/duplication_gate.mjs` | violations: 0 |
| `node scripts/quality/assert_tests_untouched.mjs master` | OK (no test code touched) |
| `CARGO_BUILD_JOBS=4 cargo nextest run --workspace` | 1252 passed / 0 failed (`.omc/logs/final-gates/nextest3.log`) |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |
| `ast-grep scan` | exit 0 |
