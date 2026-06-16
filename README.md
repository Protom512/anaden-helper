# anaden-helper

Another Eden（アナザーエデン）の自動操作ツール。Android 端末を ADB 経由で操作し、**画面認識 → アクション**のループを回す。

MAA (MaaAssistantArknights) のアプローチ（宣言的タスク・テンプレートマッチング・解像度正規化）を参考に、純 Rust で実装。

## アーキテクチャ（各層を実機検証済み）

| 層 | 方式 | 備考 |
|---|---|---|
| **Capture** | scrcpy 常駐（H.264 + openh264 デコード） | E2E 85ms（1秒仕様達成）。従来の `adb screencap` は 2034ms |
| **認識** | TM_CCOEFF_NORMED（照明不変）+ ROI + 720p(幅1280)基準座標系 | `adb` 経由の通常タッチより高精度 |
| **入力** | scrcpy control-touch（`TYPE_INJECT_TOUCH_EVENT`） | ゲームが `adb input tap` をチート対策で無視する問題を**別経路で突破** |
| **パイプライン** | 宣言的 TOML（`template`/`roi`/`algorithm`/`action`/`next`） | コード変更なしで操作フローを追加 |

## ビルド

```bash
# 通常ビルド（screencap + adb input）
cargo build --release

# scrcpy capture + touch 入力を有効化（推奨）
cargo build --release -p anaden-cli --features anaden-cli/capture-scrcpy
```

`capture-scrcpy` feature は [openh264](https://github.com/cisco/openh264) を `source` feature で自己完結ビルド（NASM 自動DL、外部 DLL 不要）。デフォルトは OFF（screencap のみで既存ビルド・テスト非破壊）。

## 準備

1. Android 端末で USB デバッグを有効化し ADB 接続（`adb devices` で確認）。
2. scrcpy-server jar をホストに配置（デフォルトは scoop インストールパス `C:\Users\<user>\scoop\apps\scrcpy\current\scrcpy-server`、`--scrcpy-jar` で上書き可）。

## 実行

```bash
target/release/anaden run <serial> <pipeline_dir> <start_task> \
  --capture scrcpy --input scrcpy --algorithm ccoeff
```

主なフラグ:

| フラグ | 値 | 説明 |
|---|---|---|
| `--capture` | `scrcpy` \| `screencap` | 画面取得方式（`scrcpy` 推奨：高速 + ゲーム入力が有効） |
| `--input` | `scrcpy` \| `adb` | 入力方式（`scrcpy` 推奨：ゲームが `adb input tap` を無視するため） |
| `--ensure-open` | `true` \| `false` | 接続時にゲーム未起動なら自動起動（デフォルト ON） |
| `--recover-launch` | `true` \| `false` | NoMatch 連続時のゲーム再起動リカバリ（デフォルト ON） |
| `--recover-nomatch-threshold` | `N` | リカバリ発動の連続 NoMatch 回数（デフォルト 5、`0` で無効） |
| `--max-iters` | `N` | 最大サイクル数 |
| `--interval` | `秒` | サイクル間隔 |

## パイプライン例

```
templates/pipelines/
  field_loop/       # field 画面認識 + 安定UI要素タップ（bottom_stable, hud_tr）
  worldmap_loop/    # ワールドマップのタブ操作（ancient_tab）
  nav_to_field/     # 日次ポップアップ dismiss → field 到達
```

TOML 1ファイル = 1タスク。`next` で状態遷移チェーンを組める。テンプレート画像と同じディレクトリに配置（パスは TOML 親ディレクトリ基準）。

## anaden-studio（テンプレート作成 GUI）

```bash
cargo run --bin anaden-studio
```

スクリーンショット上でドラッグ ROI を選ぶと、正例/負例フォルダに対する**識別力を即時検証**。バッチ混同行列も。自動収集テンプレの「安定≠識別できる」問題を、人間の目で確実な ROI を選んで解決するためのツール。

## ドキュメント

- `docs/scrcpy-protocol.md` — scrcpy v4.0 wire protocol（公式ソース実読・capture+control 仕様）
- `docs/minitouch-design.md` — minitouch 統合設計（参考。本プロジェクトでは scrcpy-touch で解決したため未使用）
- `docs/llm-wiki/` — MAA 画像認識ノウハウ + anaden-vision 再設計

## 状態（2026-06-16）

- **完全系成立**：capture（scrcpy 1秒）・認識（ccoeff）・入力（scrcpy-touch、アンチチート突破）の各層を実機検証済み。テスト 164 件 green。
- **残課題**：完全ループ（capture→認識→touch→効果）の end-to-end 実機検証、認識テンプレの更なる安定化（状態変動対策）。
