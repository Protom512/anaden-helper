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
- 試行 2 (2026-11-12 実行, 部分成功): `LoginClickLoadPc` から開始。`report.json` は
  13 iters / `fired_count=1` / `per_task_matches=[LoginClickLoadPc×1]` /
  `recovery_attempts=0` / `terminal_task=goal_timeout`。クリックは注入され、
  タイトルを離脱して街に到着した (実行後フレーム `post1.png`)。
  `goal_timeout` は `LoginWaitFieldPc` (`hud_top`) が街に無いためで想定内。
  watch が発火しなかった原因は下端クリップ (U9 追記 2) — 文字針をアイコン針に
  差し替えて解消。
- 到着判定 (試行 2 の合否条件): `hud_top` (通貨バー) は街に存在しない (DEF-008) ため、
  `login/watch/field_town_bar.toml` (`LoginFieldTownBarPc`, action=stop) を
  nav の実機検証済み town bar 針への相対参照で追加した。街/フィールド到着で
  `watch stop` → `Stop アクション到達(正常)` → exit 0。
  合否は (a) `report.json` の `fired_count >= 1` かつ `per_task_matches` に
  `LoginClickLoadPc`、(b) 実行後フレームで `LoginClickLoadPc` と
  `LoginTapTitlePc` が NoMatch (タイトルから離脱した)、(c) 実行後フレームが
  `RESULT=OK_CONTENT_CAPTURED`、(d) `watch stop: LoginFieldTownBarPc` → exit 0。
- 試行 2 の実測: (a)(b)(c) は PASS。(b) の device キャプチャは
  `LoginClickLoadPc`/`LoginTapTitlePc` とも NoMatch、variance 2022.57。(d) は
  文字針がクリップで 0.1548 だったため FAIL。アイコン針なら同じフレームで
  0.9183 → 再実行 (試行 3) で確認する。
- 試行 3 (2026-11-12 11:00-11:12, 未完了): ゲーム再起動後、クライアントが起動の
  たびに `0xC0000409` (ucrtbase.dll, FATAL_APP_EXIT) で abort する。イベントログ:
  `Faulting application AnotherEden.exe 3.17.60.0 / ucrtbase.dll / 0xc0000409`。
  ローカル LevelDB (`%LOCALAPPDATA%\Wright Flyer Studios\ANOTHER EDEN\ldb`) は
  `user/login` と `user/update_meta` の成功まで記録しており、その直後の遷移で落ちる。
  launcher 直接起動・時間をおいた再試行を含む 4 回すべて再現。ゲーム側の障害で
  ライブ再実行はブロック (コード・テンプレート側の変更では直せない)。
- 試行 3 の代替 (機械検証): `crates/anaden-engine/tests/login_watch_stop_live_frame.rs`
  が実チェーン + 実 watch def + 下端 100px を黒くした到着フレームを本番
  `PipelineDriver` に流し、`LoopStopReason::Stop` / `terminal = "watch stop:
  LoginFieldTownBarPc"` / iterations 1 / fired 0 を固定する。watch 評価・正規化・
  停止理由の経路はライブ実行と同一。

## U9. 到達 sentinel の検証 — 測定手段を 3 回間違えた (修正済み)

ライブの現フレームで `LoginFieldTownBarPc` が出ない、と判断したのが発端。実際は
**測り方が 3 通りとも間違っていた**。

- 誤り 1: `anaden-tool run-pipeline` で watch def を測った。この subcommand は
  `load_pipeline` しか呼ばない (`crates/anaden-cli/src/template_tool.rs:317`) ため
  watch def は存在せず、常に NoMatch。ここから「街ごとに針が効かない」と誤診した。
- 誤り 2: 同じ task 名の TOML を 1 ディレクトリに複数置いて閾値スイープした。
  どのファイルが読まれたか特定できず、測定値が再現しない。
- 誤り 3: 針を native 座標で切り出したまま client 座標 (1258x708) の針として使った。
  針が ROI より大きくなり、自分のフレームでも一致しない。

正しい測定は vision テスト (`load_watch_defs` + `TaskDef::detect`) だけ。この経路で
測ると、**Menu の文字だけ (client 79x30, ROI `[30,630,150,80]`, 閾値 0.80) は
5 つの街フレームで 0.85 以上、menu / title / title_menu では閾値未満**。

- 採用した変更: `templates/pipelines/nav_to_field_pc/watch/town_bar.png` を
  半透明プレート + 地形を含む 40x96 から、不透明な文字だけの 79x30 に差し替え
  (login 側は相対参照なので 1 枚)。ROI・閾値は 2 つの watch TOML を更新。
  理由は「街の地形・通知バッジに依存しない針にする」で、誤測定の撤回とは独立。
- 回帰: `town_bar_night_live_regression::day_town_live_fixture_keeps_both_town_bar_sentinels_above_threshold`
  (2026-11-12 の live フレームを追跡フィクスチャ化) + 既存 3 テスト。
- 教訓: watch def の一致は `run-pipeline` では測れない。テストで測る。

### U9 追記 2 (2026-11-12): 下端クリップで文字針が死んだ → アイコン針へ

ライブ E2E の実行後フレーム (`post1.png`, 3783x2128) は、実画面を確認しても
**ゲーム UI の下端が黒帯で切れていた** (画面キャプチャでも同じ)。この状態の
device キャプチャは content `3698x2028` になり、`Menu` 文字の下端 1/3 が欠ける。

- 実測 (`load_watch_defs` + `TaskDef::detect`, 本番前処理):
  - 文字針 (79x30): post1 で **0.1548**、追跡フレームでは 0.9857。欠けた文字は一致しない。
  - `Menu` スクロールアイコン針 (49x39, ROI `[30,600,150,100]`): post1 で **0.9183**、
    5 つの街フレームで **0.9253-0.9950**、menu 0.4973 / title 0.3839 / title_menu 0.3788。
- 採用: 針をアイコン (49x39) に差し替え、ROI を `[30,600,150,100]` に広げる
  (どちらも下端の黒帯より上)。閾値 0.80 は据え置き。
- 回帰: `clipped_bottom_capture_still_matches_both_arrival_sentinels` —
  追跡フレームの下端 100px を黒で塗り、device キャプチャのクリップを再現しても
  両 sentinel が閾値 + 0.85 バッファを超えることを固定。

### U9 追記 3 (2026-11-12): driver の watch stop 経路を統合テストで固定

`crates/anaden-engine/tests/login_watch_stop_live_frame.rs` は、実
`templates/pipelines/login` のチェーンと watch def を読み、追跡フレームの下端
100px を黒くした到着フレームを `PipelineDriver` に 1 枚だけ与える。結果は
`LoopStopReason::Stop`、`terminal = "watch stop: LoginFieldTownBarPc"`、
iterations 1、fired 0。ライブ実行と同じ正規化 (`normalize_capture`)・同じ
`TaskDef::detect`・同じ停止理由の経路を通る。

---

### E2E の合否 (プログラム判定)

1. `report.json`: `fired_count >= 1` かつ `per_task_matches` に `LoginClickLoadPc`。
2. 実行後フレームで `LoginClickLoadPc` / `LoginTapTitlePc` が両方 NoMatch
   (タイトルメニューから離脱した)。
3. 実行後フレームが「実描画」であること: `probe_windows_capture` の
   `variance` が閾値超え (`RESULT=OK_CONTENT_CAPTURED`)。
4. `watch stop: LoginFieldTownBarPc` → `Stop アクション到達(正常)` → exit 0
   (U9 追記の針で街非依存になったので、これも合否に含める)。

---

## チェック結果 (2026-11-11, このツリー)

| コマンド | 結果 |
|---|---|
| `node scripts/quality/comment_gate.mjs` | OK (325 files, no comments) |
| `node scripts/quality/complexity_gate.mjs` | violations: 0 |
| `node scripts/quality/duplication_gate.mjs` | violations: 0 |
| `node scripts/quality/assert_tests_untouched.mjs master` | OK (15 .rs file(s), no test code touched) |
| `CARGO_BUILD_JOBS=4 cargo nextest run --workspace` | 1257 passed / 0 failed / 10 skipped (`.omc/logs/live-20261111-session/nextest_workspace.log`) |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |
| `ast-grep scan` | exit 0 |
