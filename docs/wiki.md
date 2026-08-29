# Another Eden Automation Helper — プロジェクト Wiki

## 目次

1. [プロジェクト概要](#1-プロジェクト概要)
2. [アーキテクチャ](#2-アーキテクチャ)
3. [クイックスタート](#3-クイックスタート)
4. [ツールリファレンス](#4-ツールリファレンス)
5. [テンプレート作成・評価ガイド](#5-テンプレート作成評価ガイド)
6. [プラットフォーム・実機の知見](#6-プラットフォーム実機の知見)
7. [技術的な意思決定](#7-技術的な意思決定)
8. [将来の改善アイディア・検証基盤](#8-将来の改善アイディア検証基盤)

---

## 1. プロジェクト概要

Android 端末（Pixel 7a）および Windows PC 版（AnotherEden.exe）上の **アナザーエデン（Another Eden）** を自動操作・支援するツール群。

**コア機能:**
- **マルチプラットフォーム対応**: Android（ADB / scrcpy 常駐受信・タッチインジェクション）および Windows PC（`PrintWindow` キャプチャ / `SendInput` マウス合成入力）。
- **高精度画像認識**: `imageproc`（純 Rust）による正規化 SSE / TM_CCOEFF_NORMD、Needle 補間に `Lanczos3` を採用した 720p（幅 1280）基準スケーリング。
- **宣言的パイプライン駆動**: TOML ファイルで画面遷移・操作タスクを宣言し、`PipelineDriver` によるループ実行・ゴール評価・発火後検証（誠実検証）。
- **統合デスクトップ GUI（`anaden-studio`）**: ROI ドラッグ選択・正例/負例ライブスコア評価・ヒートマップ可視化・バッチ混同行列評価・パイプライン実行監視・実行履歴管理。

**開発環境:**
- 言語: Rust（edition 2024）
- Android 対象機: Google Pixel 7a（2400×1080 横画面）
- Windows 対象環境: AnotherEden.exe（1258×708 RAW クライアント領域）
- 画像認識: `imageproc` + `image`（Rayon 並列化、OpenCV C++ 非依存）

---

## 2. アーキテクチャ

### 全体構成（Cargo workspace 7 crate）

```
anaden-helper/
├── crates/
│   ├── anaden-core/        ← ドメイン型・trait・Goal 評価（副作用なし）
│   ├── anaden-device/      ← デバイス通信（Android ADB/scrcpy + Windows PC Win32 API）
│   ├── anaden-vision/      ← テンプレートマッチング・画面認識・スケーリング
│   ├── anaden-engine/      ← Sense→Think→Act メインループ・PipelineDriver・回復
│   ├── anaden-strategies/  ← ミニゲーム戦略の実装
│   ├── anaden-cli/         ← CLI エントリポイント（anaden / anaden-tool）
│   └── anaden-studio/      ← GUI ツール（ROI 選択・評価・パイプライン実行・履歴）
├── templates/              ← テンプレート画像・キャプチャ
│   ├── scenes/             ← 画面判定用テンプレート（GameState / Pipeline 別）
│   └── captures/           ← キャプチャ保存先・プローブ PNG
├── config/                 ← 設定ファイル・宣言的パイプライン TOML
└── docs/                   ← ドキュメント・Wiki・ダイアグラム
```

### ランタイムアーキテクチャ（4 信頼境界 & 10 コアコンポーネント）

> 💡 **インタラクティブ HTML ダイアグラム**: [`docs/diagrams/runtime-architecture.html`](file:///C:/Users/black/git-repo/anaden-helper/docs/diagrams/runtime-architecture.html)
> （Archify により生成されたダーク／ライトテーマ対応、章別フォーカスビュー、ズーム対応のスタンドアロン SVG/HTML）

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ [User Interface Layer]                                                      │
│   ├── anaden CLI (anaden-cli: Run / Ensure-Open / Tool)                     │
│   └── anaden-studio (anaden-studio: egui Desktop GUI & Live Logs)           │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ (Primary Path: 実行開始)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ [Automation Engine & Vision Core]                                           │
│   ├── Pipeline Driver (anaden-engine: Sense→Think→Act 駆動ループ)           │
│   │     ├─▶ Core & Goals (anaden-core: 純粋ドメイン型・Goal 評価・I/O ゼロ)  │
│   │     └─▶ Vision Engine (anaden-vision: SSE / Ccoeff / Lanczos3)          │
│   │           └─▶ Templates & Tasks (720p 基準正規化 TOML/PNG 資産)         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ (Primary Path: アクション送信)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ [Device Adaptation Layer]                                                   │
│   ├── Android Adapter (anaden-device: ADB screencap & scrcpy タッチ注入)     │
│   └── Windows Adapter (anaden-device: Win32 PrintWindow & SendInput)        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ (Primary Path: デバイス制御)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ [Target Game Environments]                                                  │
│   ├── Android (Pixel 7a: 2400×1080 横画面)                                  │
│   └── Windows PC (AnotherEden.exe: RAW 1258×708 クライアント領域)            │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Sense → Think → Act ループ

> 💡 **インタラクティブ実行シーケンス図**: [`docs/diagrams/sense-think-act.html`](file:///C:/Users/black/git-repo/anaden-helper/docs/diagrams/sense-think-act.html)
> （Archify により生成された Sense / Think / Act / Verify の一連のメッセージパッシング・ライフサイクル詳細図）

```
┌────────────────────────────────────────────────────────┐
│                   Orchestrator / Driver                │
│                                                        │
│  ┌──────────┐   ┌────────────┐   ┌──────────────────┐  │
│  │  SENSE   │──▶│   THINK    │──▶│       ACT        │  │
│  │ キャプチャ │   │ 状態判定   │   │ タップ/スワイプ/   │  │
│  │ テンプレ  │   │ 戦略/遷移  │   │ キー/マウス送信   │  │
│  │ 照合      │   │ 行動決定   │   │ 待機             │  │
│  └──────────┘   └────────────┘   └──────────────────┘  │
│       ▲                                   │            │
│       └────────── 誠実検証 / 反復 ─────────┘            │
└────────────────────────────────────────────────────────┘
```

### 依存関係の方向（絶対ルール）

```
anaden-cli ─────────┐
                    ▼
anaden-studio ──▶ anaden-engine
                    ├── anaden-core      （型定義・Goal のみ、I/O なし）
                    ├── anaden-device    （ADB / scrcpy / Win32 API）
                    ├── anaden-vision    （画像認識・スケーリング）
                    └── anaden-strategies（ミニゲーム戦略）
                          └── anaden-core
```

**内側のクレートは外側を知らない。** `anaden-core` は一切の I/O を持たない。

### 各 crate の責務

| crate | 何をするか | 何をしないか |
|---|---|---|
| `anaden-core` | `GameState`, `InputAction`, `Strategy`, `Goal` trait・型の定義 | ファイル I/O、ADB 通信、ネットワーク、Windows API |
| `anaden-device` | Android (ADB / scrcpy) & Windows (PrintWindow / SendInput) 通信・キャプチャ | ゲームロジック、画像認識 |
| `anaden-vision` | テンプレートマッチング（SSE / TM_CCOEFF_NORMD）、720p スケーリング、ROI 抽出 | 入力実行、デバイス通信 |
| `anaden-engine` | メインループ駆動、`PipelineDriver`、状態遷移、エラー回復（再起動/前景化）、誠実検証 | 直接デバイス操作・画像処理アルゴリズム実装 |
| `anaden-strategies` | ミニゲーム固有の操作ロジック | デバイス通信・画像認識の直接呼び出し |
| `anaden-cli` | CLI 引数パース、サブコマンド（`run`, `ensure-open`, `launch`, `legacy`）、ツール提供 | ビジネスロジック |
| `anaden-studio` | GUI（eframe / egui）、ROI 選択・ライブスコア、バッチ評価、パイプライン実行・ログ・履歴 | デバイス低レベルプロトコル実装 |

---

## 3. クイックスタート

### 前提条件

```bash
# 1. Rust ツールチェーン（edition 2024 対応）
rustup update stable

# 2. ADB（Android SDK Platform Tools）※ Android 実機利用時
adb version

# 3. cargo-nextest
cargo install cargo-nextest --locked
```

### ビルドとテスト

```bash
# 全ワークスペースのビルド
cargo build --workspace

# 全テストの実行
cargo nextest run --workspace

# 各バイナリの個別ビルド
cargo build --bin anaden        # メイン CLI
cargo build --bin anaden-tool   # 開発・検証ツール
cargo build --bin anaden-studio # GUI ツール
```

### デバイス接続（Android の場合）

```bash
# USB 接続後、認識確認
adb devices -l

# offline の場合
adb -s <serial> reconnect
```

### 基本操作

```bash
# 1. 宣言的パイプライン実行（Android 実機）
cargo run --bin anaden -- run config/pipelines/nav_to_field title <serial>

# 2. 宣言的パイプライン実行（Windows PC 版）
cargo run --bin anaden -- run config/pipelines/nav_to_field_pc title_pc --target windows

# 3. Studio GUI 起動
cargo run --bin anaden-studio

# 4. スクリーンショット取得
cargo run --bin anaden-tool -- capture <serial> templates/captures/test.png

# 5. テンプレートマッチングテスト（1/4 ダウンスケール）
cargo run --bin anaden-tool -- match <画像> <テンプレート> 0.85 --scale 4
```

## 4. ツールリファレンス

### `anaden`（メイン CLI）

| サブコマンド | 用途 |
|---|---|
| `run` | 宣言的パイプラインを実機 / PC でライブ実行（`PipelineDriver`） |
| `ensure-open` | ゲームの起動状態を確認し、未起動なら起動して前景化を待機（CI gate） |
| `launch` | 無条件起動コマンドを発行（リカバリ用途） |
| `legacy` | 旧来の命令型 `Orchestrator` ループ（後方互換） |

#### `anaden run` の主要オプション

```bash
cargo run --bin anaden -- run <pipeline_dir> <start_task> [serial] [オプション...]
```

- `--target <android|windows>`: 実行ターゲット（デフォルト: `android`）
- `--algorithm <sse|ccoeff>`: 認識アルゴリズム上書き
- `--interval <秒>`: ループ間隔（デフォルト: 1秒）
- `--max-iters <回数>`: 最大サイクル数（デフォルト: 100）
- `--ensure-open <bool>`: 未起動時の自動前景化（デフォルト: `true`）
- `--recover-launch <bool>`: NoMatch 連続時の自動再起動（デフォルト: `true`）
- `--verify-after-fire <bool>`: 発火後検証・誠実検証（デフォルト: `true`）
- `--goal <toml_str>` / `--goal-file <path>`: 停止ゴール条件

### `anaden-studio`（デスクトップ GUI）

```bash
cargo run --bin anaden-studio
```

- **✏️ 作成モード（Authoring）**: スクリーンショットやライブ ADB からマウスドラッグで ROI を選択。正例／負例フォルダに対する識別マージンとヒートマップをリアルタイム算出して保存。
- **📊 バッチ評価モード（Batch）**: テスト画像群に対して保存済みテンプレートを一括照合し、混同行列・正答率・感度・特異性を算出。
- **🚀 パイプラインランナー（Runner）**: 戦略・引数を選択してパイプラインを実行。stdout/stderr 分離ログビュー、リアルタイム色分け、協調的停止。
- **📜 実行履歴（History）**: 過去の実行ログ・終了コード・実行時間を永続化し一覧確認。

### `anaden-tool`（開発・デバッグ用 CLI）

| コマンド | 説明 |
|---|---|
| `capture <serial> [out]` | スクリーンショットを取得して保存 |
| `extract <img> <x> <y> <w> <h> <out>` | 画像の一部をテンプレートとして抽出 |
| `match <img> <tpl> [threshold] [-s N]` | テンプレートマッチングを実行 |
| `detect <img> [-t <dir>] [-c <threshold>]` | ディレクトリ内全テンプレートによるシーン投票判定 |
| `explore <serial> [-o <dir>] [-i N] [-d N]` | 探索的テンプレート自動収集 |
| `launch <serial>` | アナザーエデンを起動 |
| `record <serial> [dir] [-i N] [-c N]` | 連続キャプチャ |
| `run-pipeline <img> <dir> <task> [-a algo]` | パイプライン 1 ステップ Dry-run（発火なし） |

---

## 5. テンプレート作成・評価ガイド

### テンプレートのベストプラクティス

- **720p 基準正規化**: すべての座標・ROI は幅 1280 基準で保存されます。
- **小さく特徴的**: 50×50〜150×50 程度。大きすぎるとマッチング計算が重くなり、アニメーションや背景変化の影響を受けやすくなります。
- **変化に強い要素**: ボタンテキストや固有アイコンを選択（キャラ立ち絵や背景アニメーション領域を避ける）。
- **`anaden-studio` でのマージン評価**: 正例スコア最小値 - 負例スコア最大値（`margin > 0.1`）が緑色になる ROI を選定。

---

## 6. プラットフォーム・実機の知見

### Android（Google Pixel 7a）

- **解像度**: 2400×1080（横画面）
- **キャプチャ**: `adb exec-out screencap -p` を使用（Windows の CR/LF 改行破損を回避）。所要時間 ~797ms。
- **入力安定化**: ゲームが `adb input tap` を無視・ドロップする場合、`--input scrcpy`（`TYPE_INJECT_TOUCH_EVENT`）を使用可能。

### Windows PC 版（AnotherEden.exe）

- **クライアント寸法**: **1258×708 RAW 空間**（DPI スケーリング非依存）。
- **キャプチャ**: `windows-rs` による `PrintWindow`（GDI / D3D11）。
- **入力送信**: `SendInput` によるマウス合成入力。
- **アンチチート（wfsdrv）**: PC 版では `PrintWindow` / `SendInput` はブロックされず正常動作することを確認済み。
- **起動**: `Launcher.exe` 経由でプロセス監視。

### パフォーマンス実測（1ループ 1秒以内達成）

| 工程 | 実測中央値 | 割合 |
|---|---|---|
| **キャプチャ（screencap）** | ~797ms | ~86%（支配的律速） |
| **タップ送信（adb input）** | ~106ms | ~11% |
| **720p スケーリング** | ~23ms | ~3% |
| **テンプレートマッチング（ROI）** | ~4〜5ms | < 0.5% |
| **合計** | **~930ms** | **1秒以内ループを達成** |

---

## 7. 技術的な意思決定

### なぜ Rust & 純 Rust `imageproc` か
- リアルタイム自動操作に必要な速度と並行性（Rayon / Tokio）。
- OpenCV の C++ ネイティブ依存を排除し、Windows / Linux で環境構築・CI ビルドを一発で安定化。
- `VisionEngine` trait による将来的なアルゴリズム差し替えの容易性。

### Lanczos3 スケーリング
- テンプレート画像（needle）をターゲット解像度へスケーリングする際、バイリニア補間では微細な文字（version 帯等）のエッジがボケて信頼度が低下する。高品質な `Lanczos3` 補間を採用することで認識精度を大幅に向上。

### 宣言的タスクと誠実検証
- 自動操作シーケンスを TOML で宣言化し、コード変更なしでフロー追加・メンテナンスを可能に。
- アクション発火後に再度キャプチャしてテンプレート消失を確認する「誠実検証」により、偽成功を排除。

---

## 8. 将来の改善アイディア・検証基盤

- **Review Gate 定量検証基盤**: `scripts/review_gate_eval` によるレビュー精度検証ハーネス（AND コンセンサス・majority / critical-veto）。
- **常駐キャプチャの標準化**: `scrcpy` H.264 常駐受信によるキャプチャ時間の大幅短縮（~797ms → ~50ms）。
- **マルチ解像度・アスペクト比対応**: 16:9, 20:9 以外の端末への自動適応。
- **録画・リプレイ・LLM 連携**: 操作ログの学習や小型ビジョンモデルとのハイブリッド判定。

