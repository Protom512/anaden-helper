# anaden-helper

Another Eden（アナザーエデン）の自動操作ツール。**画面認識 → アクション**のループを回す。**PC 版（Windows / 16:9）専用** — Android（ADB / scrcpy）経路は Issue #188（2026-09）で削除した。

MAA (MaaAssistantArknights) のアプローチ（宣言的タスク・テンプレートマッチング・解像度正規化）を参考に、純 Rust で実装。

## アーキテクチャ（各層を実機検証済み）

| 層 | PC (Windows 16:9) |
|---|---|
| **Capture** | `Win32Capture`(PrintWindow + GetDIBits、1258x708 実測) |
| **認識** | TM_CCOEFF_NORMED + ROI。**RAW 1258x708 空間**（detect が幅1280基準へ正規化） |
| **入力** | `Win32InputExecutor`(SendInput / PostMessage) |
| **起動保証** | `Win32Launch`(プロセス起動・前景化) |
| **パイプライン** | 宣言的 TOML（`template`/`roi`/`algorithm`/`action`/`next`） |

> **座標系**: PC 版テンプレ/ROI は **RAW 1258x708 ピクセル空間**でオーサリングする。詳細は `docs/pc-capture-dimensions.md`。

## ビルド

```bash
cargo build --release
```

（旧 `capture-scrcpy` feature は Issue #188 の Android 削除に伴い廃止。PC 版は feature・ADB 不要）

## 準備

1. AnotherEden.exe をインストール（プロセス名 `AnotherEden.exe` は `Win32Capture::DEFAULT_PROCESS_NAME` で固定）。
2. **T1 ゲート（KB5094126）**: OS build 26200.8655（KB5094126 適用済み）では AnotherEden.exe が 0xC0000005 でクラッシュしライブキャプチャ不可。build 26200.8524（未適用）で実行すること（`docs/pc-capture-dimensions.md` §4）。

## 実行 — タイトルコールドスタート（リアル 1 サイクル証明）

`nav_to_field_pc` パイプラインが **TapToStartPc → LoadGamePc → FieldHudTopPc** の状態機械チェーンを `next` で繋いでいる。この順序で1サイクル駆動する:

```bash
target/release/anaden run \
  templates/pipelines/nav_to_field_pc TapToStartPc \
  --algorithm ccoeff --verify-after-fire true --max-iters 1
```

- capture/input は Win32 バックエンドへ一本化（serial 不要。旧 `--target` / `--capture` / `--input` フラグは Issue #188 で削除）。
- `--max-iters 1` で1サイクル（TapToStartPc → LoadGamePc → FieldHudTopPc 到達）で停止。
- `--verify-after-fire true`（デフォルト）で発火後に再 capture しテンプレ消失を検証（誠実検証・偽成功防止）。
- `--width` は未指定推奨（初回 capture で 1258 を実測）。DPI ズレ時のみ手動ピン。

> **状態機械の保証**: 開始タスクから終点までの到達可能性・非分岐性・厳密な3段階順序はテスト `pc_nav_to_field_one_cycle_walk_order_matches_cli_contract` / `pc_nav_to_field_cold_start_chain_is_walkable_to_field`（`crates/anaden-vision/src/pipeline.rs`）で CI 固定されている。

### 起動状態確認・起動（`ensure-open` / `launch`）

パイプライン実行なしで「ゲームが起動しているか確認し、未起動なら起動する」を単独で行う独立 CI gate サブコマンド（Issue #21）。`run` の `--ensure-open true`（デフォルト ON）と同等の起動保証ロジックを、パイプラインを回さずに実行できる。終了コードで AlreadyOpen / Launched / Timeout を機械的に取得可能（CI gate・運用スクリプトからの単体呼び出し向け）。

```bash
# 起動状態確認＋必要なら起動
target/release/anaden ensure-open

# 無条件起動（AlreadyOpen チェックなし・リカバリ用途）
target/release/anaden launch
```

**終了コード契約**（`run` とは意図的に異なる・純加算）:

| 成果物 | `ensure-open` / `launch` | `run`（参考・変更なし） |
|---|---|---|
| AlreadyOpen / Launched | `0`（成功） | soft log（継続） |
| Timeout（起動したが前景化せず） | `2`（CI gate 失敗） | soft warn（継続） |
| ハードエラー（spawn / OpenProcess 失敗） | `1` | error |

`run` は Timeout でもパイプラインを継続するが、スタンドアロン gate は CI スクリプトへ「起動失敗」を明示するため Timeout を非ゼロで返す。`--wait-secs`（既定 30）で起動/前景化待ちタイムアウトを調整可。

```bash
# CI gate 例: 起動保証を前提ステップとして実行（Timeout/ハードエラーで非ゼロ）
anaden ensure-open || { echo "起動保証失敗"; exit 1; }
```

## 主なフラグ

| フラグ | 値 | 説明 |
|---|---|---|
| `--verify-after-fire` | `true` \| `false` | 発火後検証（デフォルト ON）。発火成功後に再 capture し効果を検証・偽成功を弾く |
| `--ensure-open` | `true` \| `false` | 接続時にゲーム未起動なら自動起動（デフォルト ON） |
| `--recover-launch` | `true` \| `false` | NoMatch 連続時のゲーム再起動リカバリ（デフォルト ON） |
| `--recover-nomatch-threshold` | `N` | リカバリ発動の連続 NoMatch 回数（デフォルト 5、`0` で無効） |
| `--max-recovery-attempts` | `N` | リカバリ試行の予算上限（デフォルト 5）。健全サイクルを挟まず連続到達で打ち切り exit 1 |
| `--max-iters` | `N` | 最大サイクル数 |
| `--interval` | `秒` | サイクル間隔 |
| `--width` | `px` | device_width 手動指定（PC 版は実測推奨・未指定可） |
| `--goal` | inline TOML | 宣言的ゴール。`--goal-file` と排他。未指定時は pipeline.toml の `[[goal]]` を既定採用 |
| `--goal-file` | パス | 宣言的ゴールの TOML ファイル。`--goal` と排他。明示指定は manifest より優先 |

## パイプライン例

```
templates/pipelines/
  nav_to_field_pc/  # PC版コールドスタート: TapToStartPc → LoadGamePc → FieldHudTopPc
  field_loop_pc/    # PC版 field ループ（tap_bottom, tap_hud_tr）
  login/            # PC版ログイン
  fishing/          # 釣り（未実装タスクから参照）
```

（旧 Android 版 1280x720 空間の field_loop / nav_to_field / worldmap_loop / _title_load は Issue #188 で削除）

TOML 1ファイル = 1タスク。`next` で状態遷移チェーンを組める。テンプレート画像と同じディレクトリに配置（パスは TOML 親ディレクトリ基準）。PC 版は RAW 1258x708 空間でオーサリングする。

各ディレクトリには `pipeline.toml`（manifest）を置ける: `start_task`（最初に実行する TaskDef 名）と `[[goal]]`（終端条件）を宣言する。`anaden run` は `--goal`/`--goal-file` 未指定時、manifest の `[[goal]]` を既定ゴールとして採用する（複数 goal はいずれかで停止する Any 合成）。goal 無宣言 = 従来どおり無限ループ（`--max-iters` で打ち切り）。schema 詳細は `docs/goal-manifest.md`。

## anaden-studio（テンプレート作成 GUI）

```bash
cargo run --bin anaden-studio
```

スクリーンショット上でドラッグ ROI を選ぶと、正例/負例フォルダに対する**識別力を即時検証**。バッチ混同行列も。自動収集テンプレの「安定≠識別できる」問題を、人間の目で確かな ROI を選んで解決するためのツール。ライブキャプチャ・入力注入は Win32 バックエンド（`--exe` で対象プロセス指定可）。

## ドキュメント

- `docs/pc-capture-dimensions.md` — PC 版キャプチャ実測寸法（1258x708）と RAW 座標系不変量
- `docs/llm-wiki/` — MAA 画像認識ノウハウ + anaden-vision 再設計

## PC 版 title cold-start 再開手順（プローブ差し替え時の再検証）

実機プローブを差し替え・テンプレートを再調整したときの再検証手順。詳細な取得手順は **[Wiki: PC-Probe-Acquisition](https://github.com/Protom512/anaden-helper/wiki/PC-Probe-Acquisition)** 参照。

1. **実機タイトル停止**: `AnotherEden.exe` を起動し、タイトル画面で "Tap to Start" 点滅状態で停止させる（T1 ゲートは §「準備」参照）。
2. **プローブ取得**: PrintWindow でタイトルフレームを取得し `templates/captures/title_pc_probe.png` へ保存（生キャプチャは PC RAW 1258x708 へ in-code resize される）:

   ```bash
   cargo run --release -p anaden-device --example probe_windows_capture -- \
     --process AnotherEden.exe --out templates/captures/title_pc_probe.png
   ```
3. **E2E ゲート実行**: `cargo nextest run -p anaden-vision --features pc-e2e --run-ignored all -E 'test(pc_title_pc_templates_match_real_capture_above_threshold)'` — `title_pc_probe_path()` が規約位置のプローブを解決し、不在時は absence-skip せず fail-loud で panic する（R1 三値ゲート: `#[ignore]` + `pc-e2e` feature + `--run-ignored`）。プローブ配置後、conf >= threshold の実効ゲートとして走る。

テンプレート契約（`templates/scenes/title_pc/`・実機プローブの RAW 1258x708 空間で実測・資産契約は `tests/title_live_regression.rs` が pin）:

| ファイル | ROI [x,y,w,h] | threshold |
|---|---|---|
| `version_label.toml` | `[8,2,138,20]` | `0.80` |
| `title_logo_corner.toml` | `[604,223,160,180]` | `0.80`（アンカー `[624,263,120,120]` まわりのスラック窓） |

旧 Android 版の大型テンプレート（`title_center.png` 800x300 / `load_game_area.png` 600x150）は背景差・点滅アニメに弱い既知ブロッカーで、現行の小テンプレート設計はその迂回避決として採用された。
