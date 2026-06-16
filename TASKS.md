# TASKS

## 現在の作業（再開ポイント）

### 🔄 完全ループ(capture→認識→touch→効果)の end-to-end 実機検証
- **目的**: 完全系が実機で本当に1サイクル動くことを決定的に証明する
- **再開ポイント**: アーキテクチャ各層は個別検証済み・信頼テンプレ(`field_loop`/`worldmap_loop`, conf 0.99-1.0)も構築済み。残るは `anaden run --capture scrcpy --input scrcpy` で1タスクを実行し、発火前後screencapの**シーン判定**でアクション効果を証明すること。推論サーバ過負荷(529)で2度阻まれたため、サーバ安定後または実機復帰後に再試行。
- **完了条件**: 実機で 認識→scrcpy-touch発火→画面変化(シーン判定) の1サイクルが成立すること
- **メモ**:
  - 検証候補(効果が明確): `worldmap_loop/TapAncientTab`(タブ選択変化が最も明確)、`field_loop/TapBottomStable`/`TapHudTr`
  - 実行例: `./target/release/anaden run 33291JEHN27041 templates/pipelines/worldmap_loop TapAncientTab --capture scrcpy --input scrcpy --algorithm ccoeff --max-iters 1 --recover-launch false --ensure-open false`
  - 誠実検証必須: 効果は単発MD5でなく画面内容のシーン変化で判定(フィールドは自然変動大)

### 🔄 認識テンプレの更なる安定化（ラストマイル）
- **目的**: 全ゲームシーンで安定マッチする信頼テンプレを揃える
- **再開ポイント**: `field_loop`(bottom_stable conf~1.0, hud_tr)/`worldmap_loop`(ancient_tab)は安定。残るは title cold-start用テンプレ(大きすぎてマッチせず・要小テンプレ化)と、状態変動(時間/天候)対策。
- **完了条件**: 主要シーン(title/field/worldmap/戦闘)で誤マッチなく安定検出
- **メモ**: 単色バー(top.png conf 0.5756)の失敗教訓=**安定+特徴的要素**を選ぶ。anaden-studio(人間指示ROI作成)が本来の用途。

---

## バックログ

### 📋 アクション後検証の追加（pipeline_driver, systemic改善）
- **目的**: クリック発火後に再captureし「テンプレがまだマッチする＝アクション失敗」を検出、誤成功報告を防ぐ
- **完了条件**: `run_once` が発火後の効果を検証し、失敗を誤報告しないこと
- **メモ**: close_btn誤キャプチャ時の「閉じたと誤報」の再発防止。テンプレ品質に依存しない成功保証。実装候補: `crates/anaden-engine/src/pipeline_driver.rs::run_once`。

### 📋 title→field ナビゲーションパイプライン（コールドスタート自動化）
- **目的**: ゲーム起動直後の タイトル→ロード→field 到達を自動化
- **完了条件**: title画面からfield画面まで自動到達できること
- **メモ**: 入力層(scrcpy-touch)は解決済み。titleテンプレ(title_center/load_game_area)が大きすぎ背景差に弱い→小テンプレ化(Tap to Start ~正規化(930,488), 点滅アニメ注意)。`dismiss_daily_popup`(close_btn識別力1.0)は完成。

### 📋 README / クイックスタート整備
- **目的**: anaden のビルド・実行方法を文書化
- **完了条件**: README に build/run/flags(`--capture`/`--input`)・scrcpy-server jar 配置・feature build(`--features anaden-cli/capture-scrcpy`)手順が載ること

---

## 完了済み

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
