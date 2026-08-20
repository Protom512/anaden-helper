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

### 起動状態確認・起動（`ensure-open` / `launch`）

パイプライン実行なしで「ゲームが起動しているか確認し、未起動なら起動する」を単独で行う独立 CI gate サブコマンド（Issue #21）。`run` の `--ensure-open true`（デフォルト ON）と同等の起動保証ロジックを、パイプラインを回さずに実行できる。終了コードで AlreadyOpen / Launched / Timeout を機械的に取得可能（CI gate・運用スクリプトからの単体呼び出し向け）。

```bash
# PC 版: 起動状態確認＋必要なら起動（serial 不要）
target/release/anaden ensure-open --target windows

# Android 版: serial は位置引数（必須）
target/release/anaden ensure-open <serial> --target android

# 無条件起動（AlreadyOpen チェックなし・リカバリ用途）
target/release/anaden launch --target windows
```

**終了コード契約**（`run` とは意図的に異なる・純加算）:

| 成果物 | `ensure-open` / `launch` | `run`（参考・変更なし） |
|---|---|---|
| AlreadyOpen / Launched | `0`（成功） | soft log（継続） |
| Timeout（起動したが前景化せず） | `2`（CI gate 失敗） | soft warn（継続） |
| ハードエラー（AdbError / spawn / OpenProcess 失敗） | `1` | error |

`run` は Timeout でもパイプラインを継続するが、スタンドアロン gate は CI スクリプトへ「起動失敗」を明示するため Timeout を非ゼロで返す。

```bash
# CI gate 例: 起動保証を前提ステップとして実行（Timeout/ハードエラーで非ゼロ）
anaden ensure-open --target windows || { echo "起動保証失敗"; exit 1; }
```

> `--target windows` は Windows ビルドのみ利用可能。非 Windows ビルドでは `--target android`（ADB）のみ機能し、`windows` は graceful エラー（exit 1）する（`wfsdrv` 依存の Win32 バックエンド absent）。`--wait-secs`（既定 30）で起動/前景化待ちタイムアウトを調整可。

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
  - PC 版 title cold-start の実機 1 サイクル E2E（`title_pc_probe.png` 実機取得済み・coordinate-space verified: テスト `pc_title_pc_templates_match_real_capture_above_threshold` を >=0.80 実効ゲートへ昇格・`title_pc` ROI/threshold 実測再調整完了）。実機プローブは 2026-07-07 にキャプチャ済み。**cross-capture robustness（独立 title フレームでの conf>=0.80）は未検証** — 現状の conf=1.0 はテンプレートを当該プローブから自己クロップしたことによる自己一致であり、独立キャプチャでの再検証が残課題（Issue #12 Branch A のクローズ条件）。
  - 完全ループ（capture→認識→touch→効果）の end-to-end 実機検証、認識テンプレの更なる安定化（状態変動対策）。
  - テスト 209 件 green（`cargo nextest run --workspace`、実測値）。

### PC 版 title コールドスタート: テンプレート一覧と coordinate-space 検証の根拠（Issue #12 / Branch A 進行中）

Issue #12 は PC 版タイトルコールドスタート成立（実機 `title_pc_probe.png` 取得 → `title_pc` ROI/threshold 実測再調整 → E2E テスト `pc_title_pc_templates_match_real_capture_above_threshold` の absence-skip 解消）を追う P1 issue。実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済みで、ROI/threshold の PC RAW 1258x708 空間への実測再調整が完了。**#12 は Branch A（実機プローブ整備）のうち coordinate-space 検証までは完了**（E2E テスト 1 passed・`title_pc_probe_path()` が `Some` を返し conf >= threshold を確認）。ただし現状 conf=1.0 はテンプレートを同一プローブから自己クロップしたことによる**自己一致（self-match）** であり、**cross-capture ロバスト性（独立 title フレームでの conf>=0.80）は未検証**。独立キャプチャでの再検証まで #12 は完全クローズ扱いとしない（残課題）。

**title_pc テンプレート一覧（完成）**: `templates/scenes/title_pc/` に PC 版 16:9（RAW 1258x708）コールドスタート用の小テンプレを2つ配置済み。いずれも点滅（"Tap to Start" 正規化座標 (930,488)）に巻き込まれない固定テクスチャを小テンプレ化し、実機 20:9 の大型テンプレ（`title_center.png` 800x300 / `load_game_area.png` 600x150 等、背景差・点滅アニメに弱い TASKS.md:30-33 既知ブロッカー）の安定性問題を迂回する設計。

| ファイル | ROI [x,y,w,h] | threshold | 根拠 |
|---|---|---|---|
| `version_label.toml` | `[712,8,121,35]` | `0.80` | version/copyright 表示帯（**右上**・旧暫定 `[1046,668,112,28]`=右下 は 20:9 自己クロップ由来で不正確）。operator 実機プローブ(2026-07-07)で PC RAW 1258x708 空間へ再実測。テンプレ `version_label.png` も 1258x708 空間へ再生成済み |
| `title_logo_corner.toml` | `[624,263,120,120]` | `0.80` | title ロゴ帯の**小特徴**（operator ドラッグ全帯 `[164,63,932,405]` を `find_logo_corner_subfeature.rs` で最高テクスチャエネルギーの 120x120 窓へ再クロップ・DEFER option b）。小テンプレ上限 `roi[2]<=130 && roi[3]<=130` を満たし背景差耐性を確保。実機プローブ(2026-07-07)で PC RAW 1258x708 空間へ再実測 |

両 TOML の ROI は実機プローブ `title_pc_probe.png`（2026-07-07 キャプチャ・1918x1048 RGBA を PC RAW 1258x708 へ in-code resize）に対して実測再調整済み。E2E テスト（`--features pc-e2e --run-ignored all`）で conf >= threshold を確認済み（coordinate-space verified・cross-capture robustness は継続検証課題）。

**coordinate-space 検証成立の根拠（旧 absence-skip → R1 三値ゲート → 実機プローブ整備完了）**: かつて E2E テスト `pc_title_pc_templates_match_real_capture_above_threshold` は `title_pc_probe_path()` の None ブランチで absence-skip（検証スキップ `return`）しビルドを壊さなかった。これは PC 実機 PrintWindow キャプチャであり CI フォークやデバイス未接続環境では取得不能なため、`field_pc_probe.png` / `menu_pc_probe.png` と同じ absence-skip パターンだった。R1 で**三値ゲート（`#[ignore]` + `pc-e2e` feature + `--run-ignored`）へ移行**し、プローブ不在時は absence-skip せず fail-loud で `panic!` するよう改めた（サイレント skip が偽成功を生む懸念の排除・詳細は次節「再開手順」の R1 根拠）。通常実行（`pc-e2e` OFF）では `#[ignore]` により skipped となりビルドは壊れない。`--features pc-e2e --run-ignored all` で実効 >=0.80 ゲートとして走る。**実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済み**で `title_pc_probe_path()` が `Some` を返し、実効 >=0.80 ゲートとして走行（1 passed）。absence-skip 状態は解消済み。**ただし conf=1.0 は自己一致（self-match）であり cross-capture ロバスト性は未検証** — 独立 title フレームでの再検証まで #12 の完全クローズは留保（Branch A 進行中）。

### PC 版 title cold-start 再開手順（Step-by-step / Issue #12 Branch A 進行中・プローブ差し替え時の再検証手順）

実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済みで Branch A（実機プローブ整備・coordinate-space 検証）は完了済み。**残課題**: 独立 title フレーム（別キャプチャ）で conf>=0.80 を再確認し cross-capture ロバスト性を検証すること。以下はプローブ差し替え・再計測時の再検証手順（誰でも追える粒度で残す）。

1. **実機タイトル停止**: `AnotherEden.exe` を起動し、タイトル画面で "Tap to Start" 点滅状態で停止させる（T1 ゲート: OS build 26200.8524 未適用で実行すること・§「準備」参照）。
2. **プローブ取得**: PrintWindow でタイトルフレームを取得し、`templates/captures/title_pc_probe.png` へ保存する（`field_pc_probe.png` / `menu_pc_probe.png` と同じ規約）。生ファイルは取得ウィンドウサイズ依存（2026-07-07 キャプチャでは **1918x1048 RGBA**・オペレータ制御不可）で、pipeline.rs が PC RAW **1258x708 へ in-code resize** してから detect に渡す（ROI/template は 1258x708 空間で定義）。テンプレ再生成は `cargo run -p anaden-vision --example extract_pc_title_templates`。
3. **E2E テスト実行**: `cargo nextest run -p anaden-vision --features pc-e2e --run-ignored all -E 'test(pc_title_pc_templates_match_real_capture_above_threshold)'` を実行する（`--features pc-e2e` で feature ゲートを開き、`--run-ignored all` で `#[ignore]` マークを突破して実行）。プローブ不在時は absence-skip せず fail-loud で panic するため、プローブ配置後に初めて実効 >=0.80 ゲートとして走る。
4. **version_label 実測確認**: `templates/scenes/title_pc/version_label.toml` の `roi = [712,8,121,35]` / `threshold = 0.80` は実機プローブ（2026-07-07 計測）で conf >= 0.80 PASS を確認済み（PC RAW 1258x708 空間・右上 version 表示帯・coordinate-space verified）。プローブ差し替え時に再実行し conf >= 0.80 を再確認すること。
5. **title_logo_corner 実測確認**: `templates/scenes/title_pc/title_logo_corner.toml` の `roi = [624,263,120,120]` / `threshold = 0.80` は実機プローブ（2026-07-07 計測）で conf >= 0.80 PASS を確認済み（operator 全帯 `[164,63,932,405]` を最高エネルギー 120x120 小特徴へ再クロップ・DEFER option b・coordinate-space verified）。プローブ差し替え時に再実行し conf >= 0.80 を再確認すること。

**R1 三値ゲートと fail-loud 昇格の根拠**: `pc_title_pc_templates_match_real_capture_above_threshold` は R1 で absence-skip（None ブランチ `return`）から **3 つの状態を持つゲート** へ移行した。(1) `pc-e2e` feature OFF（通常実行）では `#[cfg_attr(not(feature = "pc-e2e"), ignore)]` により `ignored` 報告となり PASS 表記されない。(2) `pc-e2e` feature ON + `--run-ignored all` で `#[ignore]` を突破して実行されるが、プローブ不在時は `title_pc_probe_path()`（`crates/anaden-vision/src/pipeline.rs:2474`）が `None` を返し、従来の absence-skip `return` ではなく **fail-loud で `panic!`** する（namespace-dir / probe の両 early-return を廃止）。(3) `templates/captures/title_pc_probe.png`（規約位置）か workspace ルート直下の同ファイルを配置すると `title_pc_probe_path()` が `Some` を返し、プローブ実機フレーム上での conf >= threshold 実効検証へ昇格する。すなわち手順 (2) のファイル配置だけで absence-skip 状態から実効ゲートへ切り替わり、コード変更不要。歴史的経緯として、旧 absence-skip 機構は `field_pc_probe.png` / `menu_pc_probe.png` と同じパターンだったが、R1 で fail-loud に改めた（サイレント skip が偽成功を生む懸念の排除）。

**設計根拠リンク（20:9 大型テンプレ既知ブロッカーの迂回）**: 本節の小テンプレ化（`version_label` / `title_logo_corner`）は、実機 20:9 大型テンプレ（`title_center.png` 800x300 / `load_game_area.png` 600x150 等）が**背景差・点滅アニメに弱い**という既知ブロッカーを迂回する設計。根拠・詳細は前節「title_pc テンプレート一覧と coordinate-space 検証の根拠」に既出。小テンプレ化（各辺 <=130px の小テンプレ上限 `roi[2]<=130 && roi[3]<=130` を遵守）により "Tap to Start" 正規化座標 (930,488) の点滅に巻き込まれない固定テクスチャ（右上 version 帯・title ロゴ帯の小特徴）を検出対象とすることで安定性を確保している。
