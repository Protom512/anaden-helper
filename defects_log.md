# Defects Log — anaden-helper

> **履歴資料について (2026-09-13)**: Android / ADB / scrcpy 関連の欠陥記録は Issue #188
> (2026-09-10) で Android サポートを削除する以前のものです。現行は PC (Windows) 専用。


`canonical_feature_stories.md` のユーザーストーリー検証（ループB）で発見した Logic / UX 欠陥を追跡する。

| Defect ID | Affected US-ID | Test Case ID | Type | Description of Error | Steps to Reproduce | Severity | Status |
| :---: | :---: | :---: | :---: | :--- | :--- | :---: | :---: |
| DEF-001 | US-T02 | TC-T02-02 | Logic | `field_loop_pc` の `TapBottomStablePc` / `TapHudTrPc` が `field_pc_probe.png` 上で threshold 0.80 未満（ccoeff）。`scenes/field_pc` は conf=1.0 でマッチするが、パイプライン用 ROI/テンプレが実キャプチャとずれている。 | 1. `anaden-tool run-pipeline templates/captures/field_pc_probe.png templates/pipelines/field_loop_pc TapBottomStablePc --algorithm ccoeff` | High | Fixed |
| DEF-002 | US-P02 | TC-P02-02 | UX | `--target windows` で AnotherEden 未起動時、Win32 エラーが `AdbError::CommandFailed` 経由で **「ADB command failed」** と表示され原因特定が混乱する。 | 1. AnotherEden.exe を終了 2. `anaden run ... --target windows` | Medium | Verified |
| DEF-003 | US-C03 | TC-C03-01 | UX | `run-pipeline` の非マッチメッセージが「未知タスク名」を列挙するが、実際は閾値下のみ。誤解を招く。 | 1. 存在する task 名で閾値下の probe を指定 | Low | Verified |
| DEF-008 | US-PIP-01 | TC-PIP-01 | Logic | `FieldHudTopPc` 到達アンカー（通貨バー `scenes/field_pc/hud_top.png` ROI [400,0,480,100]）が街 field 状態では画面に存在しない。2026-11-10 の実キャプチャ2枚（朝: クリーン、復旧後: chrome混入モード）のいずれでも ROI 内に通貨バー無し。nav_to_field_pc のロード後到達判定が街で発火不能。 | 1. 実機で field（街）表示 2. `cargo nextest run -p anaden-vision -E 'test(FieldHudTopPc)'` 相当の detect が field_live_20261110_3783x2128.png 上で NoMatch | High | Fixed |
| DEF-009 | US-CLI-02 | TC-CLI-02 | Logic | ゲームウィンドウがモニタ矩形を超えて配置されていると（実測: window 3799x2167 at (442,442) on 3840x2160）、発火座標が画面外へ出る。`point_in_window_rect` はウィンドウ矩形（不可視領域を含む）で判定するため通過し、SendInput は OS にクランプ/無視されて game effect 無しで「発火成功」になる。verify-after-fire はこれを report.json `verify.failed=1` として記録するが、run の exit code は 0 のまま（goal 到達が exit を決める）。 | 1. ウィンドウを画面外にはみ出す位置へ置く 2. `anaden run` で発火 3. report.json verify.failed=1 かつ exit 0 | High | Fixed |
| DEF-010 | US-PIP-05 | TC-PIP-05 | Env | PrintWindow の取得モードが (a) クリーンクライアント（レンダラ健在・WM_PRINTCLIENT 経由）と (b) chrome 混入 DWM フォールバック（レンダラ停滞時、ウィンドウ全体をクライアントサイズ DC に描画・内容が ~(8,30)px シフト）で切り替わる。テンプレート資産はモード依存で、(b) で切ったテンプレは (a) でズレる。signature: キャプチャ内にタイトルバー/ウィンドウボタンが見える = (b)。2026-11-10 追加計測: (b) は letterbox crop 幅を 3783→3646 に縮小 (黒帯 ~137px)、(a) は 3773 維持。検出器 `capture_mode_verdict` (閾値 100px) landed、driver が遷移を warn。再起動直後に (b) が起きやすい (ライブ 2/2)。 | 1. ゲームをリサイズまたは停滞させる 2. probe_windows_capture を2回実行 3. バイト一致 + タイトルバー混入で (b) 判定 | Medium | Open (検出器 landed・自動復帰は後続) |
| DEF-011 | US-PIP-01 | TC-PIP-01 | Logic | nav_to_field_pc のコールドスタートが現行ビルドでライブ不能。タイトル針 (title_logo_corner) は一致するが、LoadGamePc の click_rect (600,258) が June 時代の座標で現行ロードフローと不一致の疑い + ロード中 (~30-60s) にどの針も一致せず nomatch → recovery hook がロード中にゲームを再起動し、タイトルへ戻る無限ループ (2026-11-10 ライブで 3 回再現)。 | 1. タイトル表示 2. `anaden run templates/pipelines/nav_to_field_pc TapToStartPc` 3. 発火後ロード中に recovery 再起動が走り town 到達不能 | High | Open |
| DEF-012 | US-CLI-02 | TC-CLI-02 | UX | recovery hook (`recover_launch` 既定 true) が gate 拒否 (DEF-009) や一時的エラーを「環境故障」とみなしゲームを再起動する。再起動はゲーム状態 (town→title) を破壊し、ゲーム既定配置 (442,442) がモニタ跨ぎのため再起動のたび DEF-009 状態が再現する。2026-11-10 ライブ: gate 拒否後の recovery で town セッションが失われた。 | 1. ウィンドウを画面外配置 2. `anaden run` で発火 → gate 拒否 3. recovery が再起動し town→title 化 | Medium | Fixed (clamp のみ・boot grace なし) |

## 修正メモ

- **DEF-012**: 回復を「原因 → 行動」の純関数 `plan_recovery(cause, attempt, app_alive)`
  に集約 (`anaden-engine`)。配置 fault (GateRejected) は常に `ClampWindow` で kill しない。
  Transient は attempt1=`Launch` (kill なし)、以降は生存=`WaitForExisting` / 不在=`Restart`。
  さらに `RecoveryCause::arms_boot_grace()` を Transient のみ true にし、配置 clamp の後
  180s boot grace を張らない (ライブ実測: grace が 90s goal を飲み 13 iters / 0 fires)。
  併せて `PipelineState::tick` が実行前に `current` を進める問題を修正し、execute 失敗・
  verify capture 失敗時は `current` を matching task へ戻す (戻さないと以降の全サイクルが
  未到達 task を照合し続ける)。回帰: `recovery_policy_tests.rs`,
  `execute_error_state_tests.rs`, `window_clamp_tests.rs`。
- **DEF-013**: `templates/pipelines/login/click_load.toml` (`LoginClickLoadPc`,
  template `scenes/title_pc/load_game_button.png` 192x52, roi `[500,480,300,140]`,
  threshold 0.80, `click_self`) を追加。E2E は start を明示して
  `LoginClickLoadPc` から開始する (タイトルメニュー表示済みの状態を前提)。
  タイトルタップからのフルチェーン接続は pinned test の契約改訂まで保留 (DEF-013)。
  scene def は作らない: title_pc の小テンプレ規約 (ROI ≤180px) に収めるため
  テンプレを 164px 幅へ切り詰めると confidence が 0.816 まで落ちる (floor 0.95) ため。
  回帰: `login_load_button_regression.rs` (1952x1098 追跡 fixture と 3783x2128 実機 4K
  フレーム、旧固定 rect (615,273) からの距離 > 100px を負の記録として固定)。

- **DEF-008**: nav_to_field_pc に街到達 watch レイヤーを追加 (`watch = ["FieldTownBarPc"]`、
  needle `town_bar.png` は 2026-11-10 実機タウンキャプチャの Map ボタン領域から
  cut_needle で切断・バッジ非含有)。walk 単一チェーン契約 (next.len()<=1) は維持し、
  watch をチェーン外の到達 OR レイヤーとして構造テストに登録
  (`pc_nav_to_field_cold_start_chain_is_walkable_to_field`: watch タスクは start/
  reachability 判定から免除 + action=stop を構造保証)。実機証明: タウン attach 状態から
  `anaden run templates/pipelines/nav_to_field_pc TapToStartPc` → cycle 1 で
  `watch stop: FieldTownBarPc` → exit 0 (report: nav_to_field_pc/.anaden-run/20261110-124028579)。
  回帰: `nav_town_arrival_live_regression.rs` (production preprocessing 経由 ≥0.85 buffer +
  menu/title クロススクリーン負 + 通貨バー無しの負記録)。
- **DEF-001**: `field_loop_pc` ROI/テンプレを `field_pc_probe.png` 上の実マッチ領域（`FieldPcTemplate01` / `FieldPcHudTopRight`）に合わせ再オーサリング。契約テスト `pc_field_loop_pc_templates_match_real_capture_above_threshold` を追加。
- **DEF-002**: `AdbError::CommandFailed` の表示を `Device I/O failed` に変更（Win32/ADB 共通）。
- **DEF-003**: 非マッチメッセージから「未知タスク名」を除去。
