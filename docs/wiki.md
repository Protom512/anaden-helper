# Another Eden Automation Helper — プロジェクト Wiki

## 目次

1. [プロジェクト概要](#1-プロジェクト概要)
2. [アーキテクチャ](#2-アーキテクチャ)
3. [クイックスタート](#3-クイックスタート)
4. [ツールリファレンス](#4-ツールリファレンス)
5. [テンプレート収集ガイド](#5-テンプレート収集ガイド)
6. [実機で判明した事柄](#6-実機で判明した事柄)
7. [技術的な意思決定](#7-技術的な意思決定)
8. [将来の改善アイディア](#8-将来の改善アイディア)

---

## 1. プロジェクト概要

ADB 経由で接続した Android 端末上の **アナザーエデン（Another Eden）** を自動操作するツール。

**コア機能:**
- 画面キャプチャ → テンプレートマッチングによるゲーム状態認識
- 状態に応じた自動操作（タップ・スワイプ・長押し）
- ミニゲームごとの戦略パターン（Strategy pattern）

**開発環境:**
- 言語: Rust（edition 2024）
- 対象デバイス: Pixel 7a（シリアル: `<DEVICE_SERIAL>`）
- ゲーム解像度: 2400×1080（横画面）
- 画像認識: `imageproc`（純 Rust、OpenCV 不要）

---

## 2. アーキテクチャ

### 全体構成（Cargo workspace 6 crate）

```
anaden-helper/
├── crates/
│   ├── anaden-core/        ← ドメイン型・trait（副作用なし）
│   ├── anaden-device/      ← ADB 通信（スクリーンショット・入力）
│   ├── anaden-vision/      ← テンプレートマッチング・画面認識
│   ├── anaden-engine/      ← Sense→Think→Act メインループ
│   ├── anaden-strategies/  ← ミニゲーム戦略の実装
│   └── anaden-cli/         ← CLI エントリポイント + ツール
├── templates/              ← テンプレート画像・キャプチャ
│   ├── scenes/             ← 画面判定用テンプレート（GameState 別ディレクトリ）
│   └── captures/           ← キャプチャ保存先
├── config/                 ← 設定ファイル
└── docs/                   ← ドキュメント
```

### Sense → Think → Act ループ

```
┌──────────────────────────────────────────────────┐
│                   Orchestrator                     │
│                                                    │
│  ┌─────────┐   ┌──────────┐   ┌──────────────┐  │
│  │  SENSE   │──▶│  THINK   │──▶│     ACT      │  │
│  │ キャプチャ│   │ 状態判定  │   │ タップ/スワイプ│  │
│  │ テンプレ  │   │ 戦略選択  │   │ 待機          │  │
│  │ ート照合  │   │ 行動決定  │   │              │  │
│  └─────────┘   └──────────┘   └──────────────┘  │
│       ▲                               │          │
│       └──────── フィードバック ────────┘          │
└──────────────────────────────────────────────────┘
```

### 依存関係の方向（絶対ルール）

```
anaden-cli
  └── anaden-engine
        ├── anaden-core      （型定義のみ、I/O なし）
        ├── anaden-device    （ADB 通信）
        ├── anaden-vision    （画像認識）
        └── anaden-strategies（ミニゲーム戦略）
              └── anaden-core
```

**内側のクレートは外側を知らない。** `anaden-core` は一切の I/O を持たない。

### 各 crate の責務

| crate | 何をするか | 何をしないか |
|---|---|---|
| `anaden-core` | GameState, InputAction, Strategy trait の定義 | ファイル I/O、ADB 通信、ネットワーク |
| `anaden-device` | ADB 経由のスクリーンショット取得・入力送信 | ゲームロジック、画像認識 |
| `anaden-vision` | テンプレートマッチング → GameState 変換 | 入力実行、ADB 通信 |
| `anaden-engine` | メインループの駆動・状態遷移・エラー回復 | 直接デバイス操作・画像処理 |
| `anaden-strategies` | ミニゲーム固有の操作ロジック | デバイス通信・画像認識の直接呼び出し |
| `anaden-cli` | 設定読み込み・依存関係の組み立て | ビジネスロジック |

---

## 3. クイックスタート

### 前提条件

```bash
# 1. Rust ツールチェーン
rustup update stable

# 2. ADB（Android SDK Platform Tools）
adb version

# 3. cargo-nextest
cargo install cargo-nextest --locked
```

### ビルドとテスト

```bash
# ビルド
cargo build --workspace

# テスト（32 テスト）
cargo nextest run --workspace

# ツールのみビルド
cargo build --bin anaden-tool
```

### デバイス接続

```bash
# USB 接続後、認識確認
adb devices -l

# offline の場合
adb -s <serial> reconnect
```

### 基本操作

```bash
# スクリーンショット取得
cargo run --bin anaden-tool -- capture <DEVICE_SERIAL> templates/captures/test.png

# アナザーエデン起動
cargo run --bin anaden-tool -- launch <DEVICE_SERIAL>

# 連続キャプチャ（2秒×10枚）
cargo run --bin anaden-tool -- record <DEVICE_SERIAL> templates/captures --interval 2 --count 10

# テンプレート抽出（座標指定）
cargo run --bin anaden-tool -- extract <画像> <x> <y> <w> <h> <出力>

# マッチングテスト（1/4ダウンスケール）
cargo run --bin anaden-tool -- match <画像> <テンプレート> 0.85 --scale 4
```

---

## 4. ツールリファレンス

### `anaden`（メイン CLI）

```bash
# 設定ファイル指定で起動
cargo run --bin anaden -- --device <DEVICE_SERIAL> --templates ./templates/scenes

# オプション
--device <serial>       # ADB デバイスシリアル
--templates <dir>       # テンプレートディレクトリ
--interval <ms>         # ループ間隔（デフォルト: 500ms）
--threshold <float>     # 信頼度閾値（デフォルト: 0.85）
--timeout <secs>        # 最大実行時間（0=無制限）
--config <file>         # TOML 設定ファイル
```

### `anaden-tool`（開発・デバッグ用）

| コマンド | 説明 |
|---|---|
| `capture <serial> [output]` | スクリーンショットを取得して保存 |
| `extract <img> <x> <y> <w> <h> <out>` | 画像の一部をテンプレートとして抽出 |
| `match <img> <tpl> [threshold] [--scale N]` | テンプレートマッチングを実行 |
| `launch <serial>` | アナザーエデンを起動 |
| `record <serial> [dir] [--interval N] [--count N]` | 連続キャプチャ |

---

## 5. テンプレート収集ガイド

### 手順

```
1. アナザーエデンを起動
   cargo run --bin anaden-tool -- launch <DEVICE_SERIAL>

2. 画面をキャプチャ
   cargo run --bin anaden-tool -- capture <DEVICE_SERIAL> templates/captures/screen.png

3. 特徴的な領域を抽出（50x50程度の小さい領域が推奨）
   cargo run --bin anaden-tool -- extract templates/captures/screen.png 30 15 120 35 templates/scenes/title/ver_label.png

4. マッチング精度を確認
   cargo run --bin anaden-tool -- match templates/captures/screen.png templates/scenes/title/ver_label.png 0.85 --scale 4

5. 別キャプチャで交差検証
   cargo run --bin anaden-tool -- capture <DEVICE_SERIAL> templates/captures/screen2.png
   cargo run --bin anaden-tool -- match templates/captures/screen2.png templates/scenes/title/ver_label.png 0.85 --scale 4
```

### テンプレートのベストプラクティス

- **小さく**: 50x50〜150x50 程度。大きすぎるとマッチングが重い
- **特徴的**: テキスト、アイコン、一意な UI 要素
- **変化に強い**: アニメーションしない要素を選ぶ（ボタンテキスト > キャラ絵）
- **GameState 別ディレクトリ**: `templates/scenes/title/`, `templates/scenes/battle/` 等

### 現在収集済みのテンプレート

| GameState | テンプレート | サイズ | 信頼度 |
|---|---|---|---|
| TitleScreen | `title/ver_label.png` | 120×35 | 99.3% |
| TitleScreen | `title/support_text.png` | 150×40 | 99.9% |
| TitleScreen | `title/wfs_mark.png` | 120×25 | 100% |

---

## 6. 実機で判明した事柄

### デバイス情報

| 項目 | 値 |
|---|---|
| デバイス | Google Pixel 7a |
| シリアル | `<DEVICE_SERIAL>` |
| ゲーム中の解像度 | **2400×1080（横画面）** |
| ゲームのパッケージ名 | `net.wrightflyer.anothereden` |
| メイン Activity | `net.wrightflyer.toybox.AppActivity` |
| ゲームバージョン | `ver 3.15.50 (980)` |

### ADB の注意点

| 問題 | 対策 |
|---|---|
| `adb shell screencap -p` で PNG が壊れる（Windows の CR/LF 変換） | **`adb exec-out screencap -p`** を使う |
| デバイスが `offline` になる | `adb -s <serial> reconnect` で復旧 |
| ゲーム起動時の Activity 名が `MainActivity` ではない | `net.wrightflyer.toybox.AppActivity` |

### パフォーマンス

| 測定項目 | 値 |
|---|---|
| スクリーンショット取得 | ~500ms |
| テンプレートマッチング（1/4 DS） | ~1s / テンプレート（小テンプレート） |
| フル解像度マッチング | **実用不可**（2400×1080 は遅すぎる） |

---

## 7. 技術的な意思決定

### なぜ Rust か

- 画像処理のパフォーマンス要件（リアルタイム自動操作）
- 型安全性による GameState の堅牢な管理
- 将来的に WASM やネイティブ拡張への道がある

### なぜ `imageproc`（OpenCV ではない）か

- Windows 環境で OpenCV のネイティブライブラリ依存を回避
- アナザーエデンの固定解像度 UI はテンプレートマッチングで十分
- 純 Rust で `cargo build` 一発で通る
- 精度不足が出たら `anaden-vision` 内部だけ差し替え可能

### ダウンスケール戦略

- **問題**: 2400×1080 の全画素マッチングは O(W×H×w×h) で数十分かかる
- **解決**: 1/4（600×270）に縮小してからマッチング
- **精度**: 実測 99%+。ゲーム UI の変化は 4px 精度で十分に識別可能
- **座標**: マッチ位置 × 4 で元解像度に復元。タップ精度は ±4px

---

## 8. 将来の改善アイディア

詳細は [docs/investigation.md](investigation.md) を参照。

- **Tiny LLM アプローチ**: テンプレートマッチングの代わりに超小型ビジョンモデルで画面分類
- **YouTube 動画からの学習データ収集**: プレイング動画をフレーム抽出して教師データに
- **マルチデバイス対応**: 解像度非依存のテンプレートスケーリング
- **録画・リプレイ機能**: 操作シーケンスの記録と再実行
