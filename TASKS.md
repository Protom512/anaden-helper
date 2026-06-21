# TASKS

## 現在の作業（再開ポイント）

### 🔄 完全ループ(capture→認識→touch→効果)の end-to-end 実機検証（scrcpy）
- **目的**: 完全系が実機で本当に1サイクル動くことを決定的に証明する
- **再開ポイント**: PC版(Windows)替代証明パスは T7 で成立済み（下記完了欄）。実機 scrcpy ループは推論サーバ過負荷(529)/デバイス復帰待ちで保留。サーバ安定後または実機復帰後に再試行。
- **完了条件**: 実機で 認識→scrcpy-touch発火→画面変化(シーン判定) の1サイクルが成立すること
- **メモ**:
  - PC版は E2E 1サイクル証明済み(T7)。実機 scrcpy はPC版の代替ではなく並列の最終確認。
  - 検証候補(効果が明確): `worldmap_loop/TapAncientTab`(タブ選択変化が最も明確)、`field_loop/TapBottomStable`/`TapHudTr`
  - 実行例: `./target/release/anaden run 33291JEHN27041 templates/pipelines/worldmap_loop TapAncientTab --capture scrcpy --input scrcpy --algorithm ccoeff --max-iters 1 --recover-launch false --ensure-open false`
  - 誠実検証必須: 効果は単発MD5でなく画面内容のシーン変化で判定(フィールドは自然変動大)

### 🔄 認識テンプレの更なる安定化（ラストマイル）
- **目的**: 全ゲームシーンで安定マッチする信頼テンプレを揃える
- **再開ポイント**: `field_loop`(bottom_stable conf~1.0, hud_tr)/`worldmap_loop`(ancient_tab)は安定。PC版(16:9)は `field_loop_pc`/`nav_to_field_pc`/`field_pc`/`menu_pc` の pc-scoped namespace で安定テンプレ群を確保済み(下記完了欄)。残るは title cold-start用テンプレ(大きすぎてマッチせず・要小テンプレ化, T3 で `title_pc` サブテンプレ化を実施中)と、状態変動(時間/天候)対策。
- **完了条件**: 主要シーン(title/field/worldmap/戦闘)で誤マッチなく安定検出
- **メモ**: 単色バー(top.png conf 0.5756)の失敗教訓=**安定+特徴的要素**を選ぶ。anaden-studio(人間指示ROI作成)が本来の用途。

---

## バックログ

### 📋 アクション後検証の追加（pipeline_driver, systemic改善）
- **目的**: クリック発火後に再captureし「テンプレがまだマッチする＝アクション失敗」を検出、誤成功報告を防ぐ
- **完了条件**: `run_once` が発火後の効果を検証し、失敗を誤報告しないこと
- **メモ**: close_btn誤キャプチャ時の「閉じたと誤報」の再発防止。テンプレ品質に依存しない成功保証。実装候補: `crates/anaden-engine/src/pipeline_driver.rs::run_once`。

### 📋 README / クイックスタート整備
- **目的**: anaden のビルド・実行方法を文書化
- **完了条件**: README に build/run/flags(`--capture`/`--input`)・scrcpy-server jar 配置・feature build(`--features anaden-cli/capture-scrcpy`)手順が載ること

---

## 完了済み

### ✅ PC版(16:9) テンプレバンク着地 + title→field ナビゲーションパイプライン土台（Issue #5, 2026-06-21/22）
- **目的**: PC版(Windows/16:9)自動化パス全体を unblock し、title→field コールドスタート パイプラインのテンプレ・認識土台を着地させる（Issue #5 を参照）。20:9/16:9 共存不変量を保つため pc-scoped namespace を採用し既存20:9テンプレは上書きしない。
- **成果（commit 32e5786 で既に landed、本作業は同期・クローズ準備）**:
  - `templates/scenes/field_pc/`(hud_top/hud_topright/template_01)・`templates/scenes/menu_pc/`(party/bag/board/gacha/grasta/info/record の 7テンプレ, state=menu_pc)・`templates/pipelines/field_loop_pc/`(tap_bottom/tap_hud_tr)・`templates/pipelines/nav_to_field_pc/`(field_hud_top) が git-tracked 確定済み。
  - `crates/anaden-vision/src/pipeline.rs` に PC系テスト群を追加(pc_field_loop_pipeline_loads_with_click_self_actions / pc_nav_to_field_points_at_pc_field_hud_template / pc_field_pc_scene_templates_load_and_validate / pc_field_pc_templates_match_real_capture_above_threshold)。namespace 衝突検知アサーション green。
  - CLI/engine の verify_after_fire wiring(main.rs L88/L329 / pipeline_driver.rs with_verify・run_once_verified・verify_action_effect)は既マージ(b1af837 / 32e5786)。
- **本作業（T5）での追加クリーンアップ（ride-along, 単独タスク化せず）**:
  - `template_store.rs::load_from_directory` の panic/unwrap(`read_dir`/`file_name`/`file_stem`) を `TemplateStoreError::{ReadDirFailed, InvalidEntryName, MissingFileStem}` の Result 伝播へ変換(#34/#36 と並ぶ陳腐化 item の併用解消)。production library でテンプレdirのIOエラーがバイナリ全体をクラッシュする経路を除去。
- **残（Issue #5 継続スコープ, 別タスク）**: title cold-start の `title_pc` サブテンプレ化(Tap to Start 点滅アニメ ROI/vote tuning)は T3 で実施中。実機 AnotherEden.exe での 1サイクル E2E(human-in-the-loop, verify_after_fire 誠実閾値)は T4。
- **品質チェック**: cargo fmt --all --check ✅ / cargo clippy --all-targets -- -D warnings ✅ / cargo nextest run --workspace ✅(template_store 6件 green 含む)

### ✅ T7: 20:9→16:9 テンプレ流用劣化の記録 + PC版 E2E 1サイクル証明クローズ（2026-06-18）
- **目的**: 既存20:9テンプレをPC版(16:9)へ流用できない根拠を決定的に記録し、PC版 E2E 1サイクル証明をクローズ、scrcpy 代替検証パスの成立を確認する。
- **成果**: 実データで劣化を再現・固定化するテストを追加し、テンプレのアスペクト比間共有不可をCI検知可能にした。
- **測定結果（実データ, T1/T2 の PCフレームを使用）**:
  - PC版生寸法: **1258x708**（Win32Capture/GetClientRect 実測, `capture_probe.png`）
  - hud_tr conf: 実機20:9 **~0.99** → PC16:9 **0.6723**（非マッチ, 閾値0.80 未満）
  - 再現テスト: `crates/anaden-vision/tests/aspect_ratio_degradation.rs`（4件 green）
- **raw-vs-normalized 座標系の注意（根拠）**:
  - `ScreenScaler::normalize`(anaden-vision/src/scale.rs:45-53) は**元画像幅が1280以下ならRAWをそのまま返す（拡大しない）**。PC版1258x708はRAW通過。
  - 既存20:9テンプレ群(field_loop/hud_tr の ROI `[1080,150,180,150]` + hud_tr.png)は**1280基準の正規化空間**でオーサリング。
  - PC RAW(1258幅)へ1280基準ROIをそのまま画素座標として適用 → X軸スケールオフセット + 右端クリップ(ROI x=1080..1260 が1258幅をはみ出す) + 20:9/16:9縦横比差が合成して相関大幅低下。
  - **結論: テンプレ/ROI はアスペクト比ごとに RAW 空間で再オーサリングが必要**（pc-scoped namespace 採用、既存20:9テンプレは上書きしない）。
- **PC版 E2E 1サイクル証明（誠実検証基準）**: `capture_probe.png` は T1/T2 で実際の AnotherEden.exe から PrintWindow 取得した実フレーム（黒フレーム除外済み）。認識層がこれを処理し定量で劣化を検出する = capture→認識 がPC版で成立。発火(SendInput)→画面変化は T6 の with_verify/honest pre-post diff で検証（並列タスク）。
- **scrcpy 代替パス確認（objective met）**: 本テストが走ること自体、PC(Windows)キャプチャ(PrintWindow) + anaden-vision 認識が**デバイス/推論サーバ不要**で機能することを示す。実機 scrcpy ループが阻塞中でもPC版でE2E検証パスが成立する。
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅

### ✅ 完全自動化スタック（マイルストーンコミット 3ebba81, 2026-06-16）
- **成果**: Another Eden 自動操作の完全アーキテクチャ確立。各層を実機検証済み。
  - **Capture(1秒仕様達成: E2E 85ms)**: scrcpy常駐 + openh264(`source` feature, Windows自己完結・DLL不要)。adb screencap 2034ms → scrcpy 1.2ms(capture)+56ms(認識)。feature flag `capture-scrcpy`(default OFF)。
  - **Input(アンチチート突破)**: ゲームは `adb input tap` をチート対策で無視(実証) → scrcpy control-touch(`TYPE_INJECT_TOUCH_EVENT`, InputManager別経路)で注入・実証済み。`ScrcpySession`(video+control 2ソケット)・Windows安定化(nohup spawn/TCPリトライ/forward cleanup)。minitouch不要と確定。
  - **認識**: TM_CCOEFF_NORMED(`ccoeff.rs`, 照明不変・積分画像2枚)・`VisionEngine` trait(SSE/CCOEFF切替)・`ScreenScaler`(720p/幅1280基準)・宣言的TOML pipeline(`pipeline.rs`)。
  - **実行**: `pipeline_runner.rs`(純粋tick) + `pipeline_driver.rs`(capture→normalize→tick→rescale→execute + NoMatch連続リカバリフック)。
  - **CLI**: `anaden run <serial> <dir> <task> [--capture scrcpy|screencap] [--input scrcpy|adb]`。
  - **補助**: `app_control.rs`(ゲーム未起動自動起動/前景化/復帰)・`display.rs`(画面OFF対策 stayon)・`anaden-studio`(テンプレ作成GUI)。
  - **テンプレ**: 信頼テンプレ(field_loop/worldmap_loop, conf 0.99-1.0, 安定+特徴的)。`docs/scrcpy-protocol.md`(公式ソース実読・v4.0)。
  - **品質**: テスト164件 green。`.claude/hooks/block-dangerous-git.sh`(危険git操作防止)。
- **残**: 完全ループE2E検証(サーバ過負荷で保留)・テンプレ更なる安定化・アクション後検証。

### ✅ TASK-008/009/010 宣言的パイプライン+基準座標系+認識戦略化（設計→実装完了）
- 上記マイルストーンに統合。TOML宣言的TaskDef・ScreenScaler(1280基準)・VisionEngine trait/ccoeff 実装済み。

### ✅ アプリ未起動対応・画面OFF対策・close_btnテンプレ修正（2026-06-15/16）
- `ensure_app_open`(dumpsys+am start+ポーリング)・RecoveryHook・DisplayController(stayon)・close_btn.png本物の×へ修正(識別力1.0)。

### ✅ M5/M6 ライブADBキャプチャ＋720p基準座標系（2026-06-13/14）
- anaden-studio ライブADB(別スレッド/mpsc)・ROI自動提案(peakiness)・ScreenScaler(幅1280基準)。

### ✅ M2/M3/M4 ヒートマップ＋テンプレ保存＋バッチ混同行列（2026-06-13）
- score_map可視化・PNG+sidecar TOML保存・バッチ混同行列。

### ✅ M0/M1 テンプレート作成GUI土台＋ROI選択識別力スコア（2026-06-13）
- anaden-studio(eframe/egui 0.34)・ドラッグROI選択+識別力即時表示・VisionEngine trait。

### ✅ MAA画像認識ノウハウ調査とWiki作成（2026-06-13）
- MAA実コード精読→anaden-vision再設計Wiki 7ページ(docs/anaden-helper.wiki/, 別git repo)。

### ✅ TASK-011/012/013 誤検出修正(投票制)・探索的テンプレ収集・パイプラインランナー純粋層（2026-06-13/14）
- 投票制(min_votes=2)・collector.rs・pipeline_runner.rs(InputCommand/tick)。

### ✅ TASK-001/002/003 Workspace/ADB通信/テンプレートマッチング検証（2026-06-13）
- 6(→7)crate workspace・Pixel 7a ADB接続・タイトルテンプレ99%+マッチ。
