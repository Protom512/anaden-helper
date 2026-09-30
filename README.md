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
| `--max-iters` | `N` | 最大サイクル数 |
| `--interval` | `秒` | サイクル間隔 |
| `--width` | `px` | device_width 手動指定（PC 版は実測推奨・未指定可） |

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

## anaden-studio（テンプレート作成 GUI）

```bash
cargo run --bin anaden-studio
```

スクリーンショット上でドラッグ ROI を選ぶと、正例/負例フォルダに対する**識別力を即時検証**。バッチ混同行列も。自動収集テンプレの「安定≠識別できる」問題を、人間の目で確かな ROI を選んで解決するためのツール。ライブキャプチャ・入力注入は Win32 バックエンド（`--exe` で対象プロセス指定可）。

## ドキュメント

- `docs/pc-capture-dimensions.md` — PC 版キャプチャ実測寸法（1258x708）と RAW 座標系不変量
- `docs/llm-wiki/` — MAA 画像認識ノウハウ + anaden-vision 再設計

## 状態（2026-09-13 / Issue #188 適用後）

- **PC 版（Windows 16:9）**: PrintWindow capture + SendInput 入力 + pc-scoped テンプレバンク（field_pc / menu_pc / title_pc / nav_to_field_pc / field_loop_pc）着地済み。コールドスタート状態機械（TapToStartPc → LoadGamePc → FieldHudTopPc）の到達可能性・順序は CI で固定。verify-after-fire 誠実検証 wired。
- **Android 版**: Issue #188（2026-09-10 方針・2026-09-13 適用）で削除。ADB / scrcpy / minitouch 経路と 20:9 テンプレ資産（templates/pipelines の 4 pipeline）は撤去済み。
- **残課題**:
  - PC 版 title cold-start の実機 1 サイクル E2E（`title_pc_probe.png` 実機取得済み・coordinate-space verified: テスト `pc_title_pc_templates_match_real_capture_above_threshold` を >=0.80 実効ゲートへ昇格・`title_pc` ROI/threshold 実測再調整完了）。実機プローブは 2026-07-07 にキャプチャ済み。**cross-capture robustness は title_logo_corner について Issue #208（2026-09）で実証済み**（07-07 probe 0.9848 + 09-19 独立 live キャプチャ 0.9944・`tests/title_live_regression.rs` が常時ゲート）。version_label は内容可変資産（ID/バージョン文字列が変わる）のため cross-capture 対象外とし scenes 検出専用。
  - テスト green（`cargo nextest run --workspace`・Issue #188 適用後の実測はゲート出力参照）。

### PC 版 title コールドスタート: テンプレート一覧と coordinate-space 検証の根拠（Issue #12 / Branch A 進行中）

Issue #12 は PC 版タイトルコールドスタート成立（実機 `title_pc_probe.png` 取得 → `title_pc` ROI/threshold 実測再調整 → E2E テスト `pc_title_pc_templates_match_real_capture_above_threshold` の absence-skip 解消）を追う P1 issue。実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済みで、ROI/threshold の PC RAW 1258x708 空間への実測再調整が完了。**#12 は Branch A（実機プローブ整備）のうち coordinate-space 検証までは完了**（E2E テスト 1 passed・`title_pc_probe_path()` が `Some` を返し conf >= threshold を確認）。**cross-capture ロバスト性は Issue #208（2026-09）で `title_logo_corner` アンカーにより実証済み**（07-07 probe 0.9848・自己一致 + 09-19 独立 live キャプチャ 0.9944。login/nav pipeline の検出アンカーも同アンカーへ切替済み）。`version_label` は内容可変資産（ID/バージョン文字列がキャプチャごとに変わりうる）として cross-capture 対象外・scenes 検出専用に整理した。

**title_pc テンプレート一覧（完成）**: `templates/scenes/title_pc/` に PC 版 16:9（RAW 1258x708）コールドスタート用の小テンプレを2つ配置済み。いずれも点滅（"Tap to Start" 正規化座標 (930,488)）に巻き込まれない固定テクスチャを小テンプレ化し、旧 Android 版実機の大型テンプレ（`title_center.png` 800x300 / `load_game_area.png` 600x150 等、背景差・点滅アニメに弱い TASKS.md:30-33 既知ブロッカー）の安定性問題を迂回する設計。

| ファイル | ROI [x,y,w,h] | threshold | 根拠 |
|---|---|---|---|
| `version_label.toml` | `[8,2,138,20]` | `0.80` | **左上 ID 表示帯**（Issue #182・実機 E2E run-180 で再選択）。旧 `[712,8,121,35]`=右上 version 帯から切り出したテンプレは無構造（輝度 stddev 3.76）で原理的に match 不能だった（65 iters 発火 0）。ID 帯テンプレ `version_label.png` は実機エンジンキャプチャから 138x20 で再生成し、roi も対で `[8,2,138,20]` へ更新。**Issue #208 で再々生成**（#182 版の生成元が区切り線つきオーバーレイ画面で、テンプレ下端の全幅ダーク行が純タイトル画面で conf 0.94→0.47 に崩壊し live 12/12 NoMatch。実タイトルキャプチャから再生成し roi/threshold は不変。資産契約は `tests/title_live_regression.rs` が pin）。**Issue #208 恒久修正で login/nav の検出アンカー役は終了** — ID/バージョン文字列が変わる内容可変資産で cross-capture 安定性が原理的になく（#208 再生成版も 07-07 probe には不一致）、本 TOML は scenes/title_pc 検出専用として残置（probe E2E の一致要求対象外）。さらに旧暫定 `[1046,668,112,28]`=右下 は旧 Android 版 20:9 自己クロップ由来で不正確 |
| `title_logo_corner.toml` | `[624,263,120,120]` | `0.80` | title ロゴ帯の**小特徴**（operator ドラッグ全帯 `[164,63,932,405]` を `find_logo_corner_subfeature.rs` で最高テクスチャエネルギーの 120x120 窓へ再クロップ・DEFER option b）。小テンプレ上限 `roi[2]<=180 && roi[3]<=180` (Issue #210 で 140→180 拡大・ドリフト吸収のスラック窓対応) を満たす。実機プローブ(2026-07-07)で PC RAW 1258x708 空間へ再実測。**Issue #208 恒久修正で login/nav pipeline の検出アンカーに採用**（消費者 3 TOML (および本 scenes TOML) はスラック roi `[604,223,160,180]`=アンカー原点まわり 横±20・縦 下+20/上−40 の非対称窓で共有 (#210 live で ~20px 上方ドリフトを実測したため上側ヘッドレーンを拡大)）。cross-capture 実証済み: 07-07 probe で conf 0.9848・09-19 live 実キャプチャで conf 0.9944（needle==roi 幅のぴったり ROI はドリフトで 0.78 に崩壊するためスラック必須） |

両 TOML の ROI は実機プローブ `title_pc_probe.png`（2026-07-07 キャプチャ・1918x1048 RGBA を PC RAW 1258x708 へ in-code resize）に対して実測再調整済み。E2E テスト（`--features pc-e2e --run-ignored all`）で conf >= threshold を確認済み（coordinate-space verified）。**cross-capture robustness は title_logo_corner について Issue #208 で実証済み**（07-07 probe 0.9848 + 09-19 独立 live キャプチャ 0.9944・`tests/title_live_regression.rs` が常時ゲート）。version_label は内容可変資産のため cross-capture 対象外（前段落参照）。

**coordinate-space 検証成立の根拠（旧 absence-skip → R1 三値ゲート → 実機プローブ整備完了）**: かつて E2E テスト `pc_title_pc_templates_match_real_capture_above_threshold` は `title_pc_probe_path()` の None ブランチで absence-skip（検証スキップ `return`）しビルドを壊さなかった。これは PC 実機 PrintWindow キャプチャであり CI フォークやデバイス未接続環境では取得不能なため、`field_pc_probe.png` / `menu_pc_probe.png` と同じ absence-skip パターンだった。R1 で**三値ゲート（`#[ignore]` + `pc-e2e` feature + `--run-ignored`）へ移行**し、プローブ不在時は absence-skip せず fail-loud で `panic!` するよう改めた（サイレント skip が偽成功を生む懸念の排除・詳細は次節「再開手順」の R1 根拠）。通常実行（`pc-e2e` OFF）では `#[ignore]` により skipped となりビルドは壊れない。`--features pc-e2e --run-ignored all` で実効 >=0.80 ゲートとして走る。**実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済み**で `title_pc_probe_path()` が `Some` を返し、実効 >=0.80 ゲートとして走行（1 passed）。absence-skip 状態は解消済み。**cross-capture ロバスト性は Issue #208 で `title_logo_corner` により実証済み**（probe 自己一致 0.9848 + 09-19 独立 live キャプチャ 0.9944・`TitlePcVersionLabel` は内容可変資産として本 E2E の一致要求対象外）。

### PC 版 title cold-start 再開手順（Step-by-step / プローブ差し替え時の再検証手順）

実機プローブ `title_pc_probe.png` は 2026-07-07 にキャプチャ済みで Branch A（実機プローブ整備・coordinate-space 検証）は完了済み。cross-capture ロバスト性も Issue #208（2026-09）で `title_logo_corner` アンカーにより実証済み（probe 0.9848 + 独立 live キャプチャ 0.9944）。以下はプローブ差し替え・再計測時の再検証手順（誰でも追える粒度で残す）。

1. **実機タイトル停止**: `AnotherEden.exe` を起動し、タイトル画面で "Tap to Start" 点滅状態で停止させる（T1 ゲート: OS build 26200.8524 未適用で実行すること・§「準備」参照）。
2. **プローブ取得**: PrintWindow でタイトルフレームを取得し、`templates/captures/title_pc_probe.png` へ保存する（`field_pc_probe.png` / `menu_pc_probe.png` と同じ規約）。生ファイルは取得ウィンドウサイズ依存（2026-07-07 キャプチャでは **1918x1048 RGBA**・オペレータ制御不可）で、pipeline.rs が PC RAW **1258x708 へ in-code resize** してから detect に渡す（ROI/template は 1258x708 空間で定義）。テンプレ再生成は `cargo run -p anaden-device --example resize_crop_template`（デフォルト roi は Issue #182 の左上 ID 帯 `8,2,138,20`。旧 `extract_pc_title_templates` は 2026-07-07 の version 帯抽出用で現行アンカーには非対応）。
3. **E2E テスト実行**: `cargo nextest run -p anaden-vision --features pc-e2e --run-ignored all -E 'test(pc_title_pc_templates_match_real_capture_above_threshold)'` を実行する（`--features pc-e2e` で feature ゲートを開き、`--run-ignored all` で `#[ignore]` マークを突破して実行）。プローブ不在時は absence-skip せず fail-loud で panic するため、プローブ配置後に初めて実効 >=0.80 ゲートとして走る。
4. **version_label 実測確認**: `templates/scenes/title_pc/version_label.toml` の `roi = [8,2,138,20]` / `threshold = 0.80` は実機 E2E（Issue #182・run-180-live-e2e・PC RAW 1258x708 空間・**左上 ID 表示帯**）で発火 1/10 iters・フィールド到達を確認済み。テンプレ `version_label.png` は同キャプチャから 138x20 で再生成された対更新品。**Issue #208（2026-09-19）で実タイトルキャプチャ（live PrintWindow 1952x1098・`.omc/logs/routine-e2e-0918c/uc2-shot-000.png`）から再々生成** — #182 版は生成元画面（区切り線つきオーバーレイ）の全幅ダーク行を含み、純タイトル画面で conf 0.94→0.47 に崩壊していた（live 12/12 NoMatch の根因）。**Issue #208 恒久修正で本テンプレは login/nav アンカー役を終了し scenes 検出専用**（内容可変資産のため probe E2E の一致要求対象外・資産契約は `tests/title_live_regression.rs` が pin）。プローブ差し替え時に再実行し conf >= 0.80 を再確認すること。
5. **title_logo_corner 実測確認**: `templates/scenes/title_pc/title_logo_corner.toml` の `roi = [604,223,160,180]` (スラック窓・anchor [624,263,120,120] まわり) / `threshold = 0.80` は実機プローブ（2026-07-07 計測）で conf 0.9848 PASS を確認済み（operator 全帯 `[164,63,932,405]` を最高エネルギー 120x120 小特徴へ再クロップ・DEFER option b・coordinate-space verified）。**Issue #208 恒久修正で login/nav pipeline の検出アンカーに採用**: 消費者 3 TOML（`login/tap_title`・`nav_to_field_pc/tap_to_start`・`nav_to_field_pc/load_game`）はスラック roi `[604,223,160,180]`（横±20・縦 下+20/上−40 の非対称窓・#210 live 上方 ~20px ドリフト実測で上側を広く）で共有する（needle==roi 幅のぴったり ROI はキャプチャ間ドリフト実測 (+2,-1) px で conf 0.99→0.78 に崩壊するため）。cross-capture 実証: probe（自己一致）0.9848 + 09-19 独立 live キャプチャ 0.9944（`tests/title_live_regression.rs`・`pc_login_tap_title_matches_title_probe` がゲート）。プローブ差し替え時に再実行し conf >= 0.80 を再確認すること。

**R1 三値ゲートと fail-loud 昇格の根拠**: `pc_title_pc_templates_match_real_capture_above_threshold` は R1 で absence-skip（None ブランチ `return`）から **3 つの状態を持つゲート** へ移行した。(1) `pc-e2e` feature OFF（通常実行）では `#[cfg_attr(not(feature = "pc-e2e"), ignore)]` により `ignored` 報告となり PASS 表記されない。(2) `pc-e2e` feature ON + `--run-ignored all` で `#[ignore]` を突破して実行されるが、プローブ不在時は `title_pc_probe_path()`（`crates/anaden-vision/src/pipeline.rs`）が `None` を返し、従来の absence-skip `return` ではなく **fail-loud で `panic!`** する（namespace-dir / probe の両 early-return を廃止）。(3) `templates/captures/title_pc_probe.png`（規約位置）か workspace ルート直下の同ファイルを配置すると `title_pc_probe_path()` が `Some` を返し、プローブ実機フレーム上での conf >= threshold 実効検証へ昇格する。すなわち手順 (2) のファイル配置だけで absence-skip 状態から実効ゲートへ切り替わり、コード変更不要。歴史的経緯として、旧 absence-skip 機構は `field_pc_probe.png` / `menu_pc_probe.png` と同じパターンだったが、R1 で fail-loud に改めた（サイレント skip が偽成功を生む懸念の排除）。

**設計根拠リンク（旧 Android 版大型テンプレ既知ブロッカーの迂回）**: 本節の小テンプレ化（`version_label` / `title_logo_corner`）は、旧 Android 実機の大型テンプレ（`title_center.png` 800x300 / `load_game_area.png` 600x150 等）が**背景差・点滅アニメに弱い**という既知ブロッカーを迂回する設計。根拠・詳細は前節「title_pc テンプレート一覧と coordinate-space 検証の根拠」に既出。小テンプレ化（各辺 <=140px の小テンプレ上限 `roi[2]<=140 && roi[3]<=140` を遵守・Issue #182 の ID 帯 138px 参照）により "Tap to Start" 正規化座標 (930,488) の点滅に巻き込まれない固定テクスチャ（左上 ID 表示帯・title ロゴ帯の小特徴）を検出対象とすることで安定性を確保している。
