# TASKS

## 現在の作業（再開ポイント）

### ✅ TASK-003 テンプレートマッチング画像認識の検証
- **完了日時**: 2026-06-13
- **成果**: Pixel 7a でタイトル画面のテンプレート3種を抽出し、クロスキャプチャで 99%+ の信頼度でマッチ成功
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **詳細**:
  - ダウンスケール（1/4）導入で 2400x1080→600x270 に縮小してマッチング。高速化を実現
  - `TemplateMatcher` に `downscale_factor` パラメータ追加。座標は元解像度に自動逆変換
  - `anaden-tool` バイナリ追加: capture / extract / match / launch / record コマンド
  - タイトル画面テンプレート3種: ver_label, support_text, wfs_mark（templates/scenes/title/）

---

## バックログ

### 🔄 TASK-004 SceneDetector（画面→GameState変換）の検証
- **目的**: 複数テンプレートを用いて、実際のゲーム画面がどの GameState に該当するかを判定する
- **再開ポイント**: TASK-003 完了。ダウンスケール付きテンプレートマッチングが実機で動作確認済み。次は複数画面のテンプレートを収集して SceneDetector の精度を検証する
- **完了条件**: タイトル画面・ホーム画面・バトル画面のテンプレートで正しく判定できること
- **メモ**: `anaden-tool record` で連続キャプチャしながらゲーム操作してテンプレートを収集する
- **目的**: 複数テンプレートを用いて、実際のゲーム画面がどの GameState に該当するかを判定する
- **完了条件**: タイトル画面・ホーム画面・バトル画面のテンプレートで正しく判定できること

### 📋 TASK-005 Orchestrator メインループの実機検証
- **目的**: Sense→Think→Act ループを実機で回す
- **完了条件**: 手動でテンプレートを登録した状態で、自動ループが回って画面状態に応じたログ出力ができること

### 📋 TASK-006 CLI エントリポイントの動作確認
- **目的**: `anaden-cli` から起動してデバイスに接続できることを確認
- **完了条件**: `cargo run --package anaden-cli -- --device <serial>` で起動して、デバイス接続メッセージが表示されること

### 📋 TASK-007 テンプレート画像の収集と認識精度検証
- **目的**: 実際のアナザーエデン画面からテンプレート画像を収集し、認識精度を実機で検証する
- **完了条件**: 主要画面（タイトル・ホーム・バトル）のテンプレートで 90% 以上の認識率を達成

---

## 完了済み

### ✅ TASK-001 Workspace & Core型の実装
- **完了日時**: 2026-06-13
- **成果**: 6 crate の Cargo workspace + ドメイン型が全て定義済み。31テスト全通過
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅

### ✅ TASK-002 ADB デバイス通信層の実装と実機確認
- **完了日時**: 2026-06-13
- **成果**: Pixel 7a にADB接続してスクリーンショット取得・アプリ起動が動作確認済み
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **重要な発見**:
  - **デバイス**: Pixel 7a。シリアル: `<DEVICE_SERIAL>`
  - **画面解像度**: ゲーム中は **2400x1080（横画面）**。config の `capture_resolution` を修正要
  - **パッケージ名**: `net.wrightflyer.anothereden`
  - **メインActivity**: `net.wrightflyer.toybox.AppActivity`
  - **スクリーンショット**: `exec-out screencap -p` 必須（`shell` だと CR/LF で PNG が壊れる）
