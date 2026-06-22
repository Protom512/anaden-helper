# anaden-helper

Another Eden（アナザーエデン）の自動操作ツール。**画面認識 → アクション**のループを回す。**2 つのターゲット**をサポートする:

- **PC 版（Windows / 16:9）** — `Win32Capture`(PrintWindow) + `Win32InputExecutor`(SendInput)。ADB 不要。
- **Android 版（20:9）** — ADB 経由。scrcpy 常駐 capture + scrcpy control-touch（アンチチート突破）。

MAA (MaaAssistantArknights) のアプローチ（宣言的タスク・テンプレートマッチング・解像度正規化）を参考に、純 Rust で実装。

## アーキテクチャ（各層を実機検証済み）

| 層 | Android (20:9) | PC (Windows 16:9) |
|---|---|---|
| **Capture** | scrcpy 常駐（H.264 + openh264 デコード、E2E 85ms） | `Win32Capture`(PrintWindow + GetDIBits、1258x708 実測) |
| **認識** | TM_CCOEFF_NORMED + ROI + 幅1280 基準正規化座標系 | 同じ TM_CCOEFF_NORMED。**RAW 1258x708 空間**（正規化は幅<=1280 でパススルー） |
| **入力** | scrcpy control-touch（`TYPE_INJECT_TOUCH_EVENT`） | `Win32InputExecutor`(SendInput / PostMessage) |
| **起動保証** | `app_control.rs`(ADB `am start`) | `Win32Launch`(プロセス起動・前景化) |
| **パイプライン** | 宣言的 TOML（`template`/`roi`/`algorithm`/`action`/`next`） | 同じ（pc-scoped namespace で 20:9 と共存） |

> **座標系の重要な違い**: Android 版テンプレ/ROI は幅1280基準の**正規化座標系**、PC 版は**RAW 1258x708 ピクセル空間**。そのため PC 版テンプレは pc-scoped namespace（`*_pc/`）で別途用意し、20:9 版を上書きしない。詳細は `docs/pc-capture-dimensions.md`。

## ビルド

```bash
# 通常ビルド（Android: screencap + adb input）
cargo build --release

# scrcpy capture + touch 入力を有効化（Android 推奨）
cargo build --release -p anaden-cli --features anaden-cli/capture-scrcpy
```

`capture-scrcpy` feature は [openh264](https://github.com/cisco/openh264) を `source` feature で自己完結ビルド（NASM 自動DL、外部 DLL 不要）。デフォルトは OFF。PC 版（`--target windows`）は feature 不要・ADB 不要で動作する。

## 準備

### PC 版（Windows）

1. AnotherEden.exe をインストール（プロセス名 `AnotherEden.exe` は `Win32Capture::DEFAULT_PROCESS_NAME` で固定）。
2. **T1 ゲート（KB5094126）**: OS build 26200.8655（KB5094126 適用済み）では AnotherEden.exe が 0xC0000005 でクラッシュしライブキャプチャ不可。build 26200.8524（未適用）で実行すること（`docs/pc-capture-dimensions.md` §4）。

### Android 版

1. Android 端末で USB デバッグを有効化し ADB 接続（`adb devices` で確認）。
2. scrcpy-server jar をホストに配置（デフォルトは scoop インストールパス `C:\Users\<user>\scoop\apps\scrcpy\current\scrcpy-server`、`--scrcpy-jar` で上書き可）。

## 実行

### PC 版 — タイトルコールドスタート（リアル 1 サイクル証明）

`nav_to_field_pc` パイプラインが **TapToStartPc → LoadGamePc → FieldHudTopPc** の状態機械チェーンを `next` で繋いでいる。この順序で1サイクル駆動する:

```bash
target/release/anaden run --target windows \
  templates/pipelines/nav_to_field_pc TapToStartPc \
  --algorithm ccoeff --verify-after-fire true --max-iters 1
```

- `--target windows` で capture/input を Win32 バックエンドへ一本化（serial 不要）。
- `--max-iters 1` で1サイクル（TapToStartPc → LoadGamePc → FieldHudTopPc 到達）で停止。
- `--verify-after-fire true`（デフォルト）で発火後に再 capture しテンプレ消失を検証（誠実検証・偽成功防止）。
- `--width` は未指定推奨（初回 capture で 1258 を実測）。DPI ズレ時のみ手動ピン。

> **状態機械の保証**: 開始タスクから終点までの到達可能性・非分岐性・厳密な3段階順序はテスト `pc_nav_to_field_one_cycle_walk_order_matches_cli_contract` / `pc_nav_to_field_cold_start_chain_is_walkable_to_field`（`crates/anaden-vision/src/pipeline.rs`）で CI 固定されている。

### Android 版

```bash
target/release/anaden run <serial> <pipeline_dir> <start_task> \
  --capture scrcpy --input scrcpy --algorithm ccoeff
```

## 主なフラグ

| フラグ | 値 | 説明 |
|---|---|---|
| `--target` | `android` \| `windows` | 実行ターゲット（デフォルト `android`）。`windows` で PC 版 Win32 バックエンド（serial 不要） |
| `--capture` | `scrcpy` \| `screencap` | 画面取得方式（Android 版。PC 版は Win32 へ自動切替） |
| `--input` | `scrcpy` \| `adb` | 入力方式（Android 版。ゲームが `adb input tap` を無視するため `scrcpy` 推奨） |
| `--verify-after-fire` | `true` \| `false` | 発火後検証（デフォルト ON）。発火成功後に再 capture し効果を検証・偽成功を弾く |
| `--ensure-open` | `true` \| `false` | 接続時にゲーム未起動なら自動起動（デフォルト ON） |
| `--recover-launch` | `true` \| `false` | NoMatch 連続時のゲーム再起動リカバリ（デフォルト ON） |
| `--recover-nomatch-threshold` | `N` | リカバリ発動の連続 NoMatch 回数（デフォルト 5、`0` で無効） |
| `--max-iters` | `N` | 最大サイクル数 |
| `--interval` | `秒` | サイクル間隔 |
| `--width` | `px` | device_width 手動指定（PC 版は実測推奨・未指定可） |

## パイプライン例

```
templates/pipelines/
  nav_to_field_pc/  # PC版コールドスタート: TapToStartPc → LoadGamePc → FieldHudTopPc
  field_loop_pc/    # PC版 field ループ（tap_bottom, tap_hud_tr）
  field_loop/       # Android版 field 認識 + 安定UIタップ（bottom_stable, hud_tr）
  worldmap_loop/    # Android版 ワールドマップのタブ操作（ancient_tab）
  nav_to_field/     # Android版 日次ポップアップ dismiss → field 到達
```

TOML 1ファイル = 1タスク。`next` で状態遷移チェーンを組める。テンプレート画像と同じディレクトリに配置（パスは TOML 親ディレクトリ基準）。PC 版（`*_pc/`）は RAW 1258x708 空間でオーサリングする。

## anaden-studio（テンプレート作成 GUI）

```bash
cargo run --bin anaden-studio
```

スクリーンショット上でドラッグ ROI を選ぶと、正例/負例フォルダに対する**識別力を即時検証**。バッチ混同行列も。自動収集テンプレの「安定≠識別できる」問題を、人間の目で確かな ROI を選んで解決するためのツール。

## ドキュメント

- `docs/pc-capture-dimensions.md` — PC 版キャプチャ実測寸法（1258x708）と RAW 座標系不変量
- `docs/scrcpy-protocol.md` — scrcpy v4.0 wire protocol（公式ソース実読・capture+control 仕様）
- `docs/minitouch-design.md` — minitouch 統合設計（参考。本プロジェクトでは scrcpy-touch で解決したため未使用）
- `docs/llm-wiki/` — MAA 画像認識ノウハウ + anaden-vision 再設計

## 状態（2026-06-22）

- **PC 版（Windows 16:9）**: PrintWindow capture + SendInput 入力 + pc-scoped テンプレバンク（field_pc / menu_pc / title_pc / nav_to_field_pc / field_loop_pc）着地済み。コールドスタート状態機械（TapToStartPc → LoadGamePc → FieldHudTopPc）の到達可能性・順序は CI で固定。verify-after-fire 誠実検証 wired。
- **Android 版（20:9）**: 完全系成立。capture（scrcpy 1秒）・認識（ccoeff）・入力（scrcpy-touch、アンチチート突破）の各層を実機検証済み。
- **残課題**:
  - PC 版 title cold-start の実機 1 サイクル E2E（`title_pc_probe.png` 実機取得後、absence-skip テスト `pc_title_pc_templates_match_real_capture_above_threshold` を >=0.80 実効ゲートへ昇格）。現在はデバイス未接続のため absence-skip を維持。
  - 完全ループ（capture→認識→touch→効果）の end-to-end 実機検証、認識テンプレの更なる安定化（状態変動対策）。
  - テスト 205 件 green（`cargo nextest run --workspace`）。
