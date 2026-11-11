# 深化候補レビュー (2026-11-11)

アーキテクチャレビュー記録。目的は「浅いモジュールを深いモジュールへ」——
インタフェースの向こうに多くの振る舞いを隠し、locality（変更・バグ・知識の集中）と
leverage（呼び出し側が得るもの）を上げること。

- 対象: `crates/` 配下 7 crate
- 方法: 4 領域（engine / vision / studio+cli / core+device+strategies）の並行調査 +
  `rg` による裏取り + 品質ゲート実測
- 用語: **Module**（インタフェースと実装を持つもの）/ **Interface**（呼び出し側が知るべき
  すべて）/ **Depth**（小さなインタフェースの背後の振る舞い量）/ **Seam**（差し替え点）/
  **Adapter** / **Locality** / **Leverage** / **Deletion test**（削除したら複雑さは
  消えるか、呼び出し側に再出現するか）

> この文書は判断の記録であり、実装計画ではない。採否は未決定。

---

## 1. 計測値

| ファイル | 総行数 | うちテスト | 備考 |
|---|---:|---:|---|
| `anaden-engine/src/pipeline_driver.rs` | 5536 | 4032 (73%) | 105 テスト。`PipelineDriver` は 21 フィールド / 36 メソッド / 7 位置引数 ctor |
| `anaden-vision/src/pipeline.rs` | 4218 | 3614 (86%) | 生産コードは 604 行に 9 責務 |
| `anaden-studio/src/` | 51 ファイル / 15.8k | — | `pub mod` 49 個、うち 7 個はバレル（計 43 行） |
| `anaden-studio/src/runner_exec.rs` | 1645 | 1011 (61%) | 生産 638 行に spawn / argv 文法 / JSON / 履歴 / 設定 |
| `anaden-cli/src/main.rs` | 1433 | 512 | `Run` は 20 フィールド、18 引数を 3 関数で受け渡し |

補助計測（`rg` 実測）:

| 項目 | 値 |
|---|---|
| `TaskDef { .. }` リテラル構築 | 76 箇所 / 22 ファイル |
| `PipelineDriver::new(` | 46 箇所（うち 41 は自身のテスト） |
| `fn task_def(` テストフィクスチャ | studio 5 ファイルに逐語コピー |
| `fn loop_goal(` テストフィクスチャ | studio 4 ファイルに逐語コピー |
| `FakeCapture` 定義 | 3 箇所（engine, cli/e2e, cli/main） |
| `#[cfg(windows)]` 出現 | cli + studio の 9 ファイル |
| `anaden-device` / `anaden-core` の `pub trait` | 0 件 |

---

## 2. 品質ゲート実測（2026-11-11）

```
$ node scripts/quality/complexity_gate.mjs
  functions: 3619 total = 1630 production + 1968 test-excluded + 21 bench
  test regions: 82 in scanned files; 39 tests/ files excluded wholesale
  over-threshold inside test regions (excluded): 1
    crates/anaden-vision/src/pipeline.rs:2059 ... cyc=20 cog=36
  violations: 0   EXIT=0
```

```
$ node scripts/quality/duplication_gate.mjs
  pairs: 105 total = 85 production + 19 test-excluded + 1 bench
  allowlisted (exempt false positives): 22
  advisory (0.80-0.84, informational): 63
  violations: 0   EXIT=0
```

**読み取り 2 点:**

1. **複雑度ゲートはテスト領域を除外する。** したがって「生産コードが 604 行、テストが
   3614 行」というファイルは、ゲート上は健全に見える。テスト量は構造の症状であって
   ゲートの指標ではない (`82` test regions / `39` tests ファイル丸ごと除外)。
2. **重複ゲートの green は 22 組の除外に依存している。** `duplication-allowlist.json` は
   24 エントリ、うち **19 組が `app_ui_authoring.rs` の 1 ファイル内**（section 関数同士が
   85–91% 類似）。残り 3 組は `run_pipeline_live`↔`run_with_windows` (93.18%)、
   `resolve_run_artifacts`↔`resolve_routine_artifacts` (94.04%)、
   `link_saved_pipeline`↔`ui_task_link` (90.34%)。除外理由はいずれも「実読判定で偽陽性」と
   して書かれているが、**除外が 1 ファイルに 19 組集中している**こと自体が、そのファイルの
   反復構造（ほぼ同型の section 関数 8 本）を示している。

---

## 3. 候補

### 1. 「パイプライン」をモジュールにする — `TaskDef` が TOML 行と実行時タスクを兼ねる

- **Files**: `anaden-vision/src/pipeline.rs:18-603`, `scale.rs`, `diagnose.rs:41-125`
- **Problem**: 生産 604 行に 9 責務（TOML スキーマ / 手書き serde `:56-160` / 検証 /
  ロード / 保存 / プロセス全体 `static TEMPLATE_CACHE:357` / マッチング / ステップ実行 /
  クロップ）。`TaskDef` は 9 フィールドで 2 つの意味を持つ: 相対パス（TOML 行）と、
  `load_pipeline:415-418` が絶対化した後の実行時パス。よって `run_step:577` は
  `detect(screenshot, Path::new(""))` と引数を捨てるしかない。`base` は生産コードで未読。
- **Solution**: `TaskDef` を TOML 1 行の形に純化し、ランタイムに `Pipeline`
  （タスク集合 + 名前索引 + テンプレート根 + `step(current, frame)`）を置く。索引と
  パス解決は内側に隠す。キャッシュは独立モジュールへ。
- **Benefits**: *Locality* スキーマ変更が 1 ファイルで終わる（現在 14 studio ファイル +
  engine + cli に波及）。*Leverage* 呼び出し側は 9 フィールドもパス解決規則も知らない。
  *Tests* 3614 行のテスト塊が継ぎ目で割れる。リポジトリ資産/README を読むテスト (~24 件)
  を `tests/` へ出せる。

### 2. ステップ結果と停止理由を 1 語彙に統合

- **Files**: `pipeline_runner.rs:7-110`, `pipeline_driver.rs:389-412,833-928`,
  `routine_runner.rs:107-193`, `cli/contract.rs:46-56`, `studio/runner_exec.rs:162`,
  `studio/log_status.rs:106`
- **Problem**: 1 サイクルの結果が 4 型（`TickResult` / `anaden_vision::StepOutcome` 構造体 /
  `anaden_engine::StepOutcome` enum（同名異義）/ `StepStatus`）。同じ流れを `tick`,
  `run_once`, `check_watch:638-690` が 3 回実装。`LoopStopReason` の 9 変数を 4 箇所が
  別々に match し、日本語文言がバイト単位で重複。
- **Solution**: 結果を 1 型（コマンド + 次タスク + 信頼度 + 領域）に。状態機械は engine の
  1 実装へ。停止理由は enum 自身に label / exit_code / is_success を持たせる。
- **Benefits**: *Locality* 停止理由 1 個追加が 5 ファイル→1。*Leverage* 表示側は
  `reason.label()`。*Tests* 「全変数網羅」テストが 4 本→1 テーブルテスト。
- **Duplicate gate 根拠**: `run_pipeline_live`↔`run_with_windows` 93.18% は allowlist 済み。

### 3. `PipelineDriver` の構築を 1 つの値に

- **Files**: `pipeline_driver.rs:414-514`, `cli/main.rs:440-490,545-556`,
  `cli/routine.rs:242-276`
- **Problem**: 7 位置引数（`300` が cli 3 + 自身のテスト 41 箇所）+ 7 ビルダ。
  `last_match`(590→1299) と `verify_stats`(716→1489) が `&mut self` 経由の裏口チャネル。
  `snapshot_counter` は非リセット、`verify_stats` はループ毎リセットと寿命規約が 2 種。
- **Solution**: 実行ポリシー（スワイプ時間 / 誠実検証 / ジッタ / watch / リカバリ予算 /
  キャンセル）を既定値付き 1 値にし、`new(device, pipeline, start, policy)`。裏口は戻り値へ。
- **Benefits**: *Leverage* `#[allow(clippy::too_many_arguments)]` 3 箇所が消える。
  *Locality* 既定値が 1 箇所。*Tests* `build_*_driver` 5 ヘルパが 1 つに。

### 4. デバイスの継ぎ目が宣言されている場所が違う

- **Files**: `anaden-device/src/lib.rs:19-40`, `pipeline_driver.rs:324-348`,
  `cli/main.rs:376-393`, `studio/source.rs:12-92`, `studio/authoring_ui.rs:63`
- **Problem**: `anaden-device` に trait が 1 つも無い。`Capture`/`Input` は消費側の engine が
  他クレート型への impl として `#[cfg(windows)]` 付きで宣言。結果 `Win32*` 直接構築が
  cli + studio に 10 箇所、`cfg(windows)` が 9 ファイルへ、「unsupported OS」文字列 5 コピー。
  `ensure.rs` の `NotReady` は device からは返されず cli だけが生成。
- **Solution**: 継ぎ目を `anaden-device` に移し、キャプチャ/入力/起動/起動保証を 1 境界の
  背後へ。`Win32Device` が唯一のアダプタ、フェイクが第 2 アダプタ（＝実在する継ぎ目）。
- **Benefits**: *Locality* OS 分岐とスタブが 1 crate に閉じる。*Leverage* 上位はデバイスだけ
  知る。*Tests* `FakeCapture` 3 重定義が解消。

### 5. 座標空間に型が無い — 変換 5 実装、1258×708 が 3 定義

- **Files**: `vision/scale.rs:4-94`, `vision/letterbox.rs:10-15`,
  `engine/pipeline_driver.rs:293-307`, `studio/authoring_coords.rs:49-75`,
  `studio/scenario_validate.rs:7-8`, `vision/diagnose.rs:30-125`
- **Problem**: RAW 1258×708 / base-1280 / letterbox キャンバス / ウィジェット座標がすべて
  同じ `ScreenRegion` で流れる。`normalize:44` は幅のみ等倍、`roi_to_normalized:66-80` は
  x/y 別比、`rescale_command:293` は `ScreenScaler` を使わず `CropInfo` から式を再構成、
  studio `from_base:67` が 3 つ目、`diagnose_task` が `TaskDef::detect` を丸ごと再実装。
  `BASE_HEIGHT` は `#[allow(dead_code)]` のまま 720 はリテラルで散在。
  `Action::ClickRect` の ROI は RAW、`matched_region` は正規化空間 — 型は何も語らない。
- **Solution**: フレーム幾何（RAW サイズ + クロップ + 基準サイズ）を 1 値にし、
  `raw_to_base` / `base_to_raw` をそのメソッドに。キャプチャが生成して明示的に持ち回る。
  RAW 矩形と基準矩形を別型に。
- **Benefits**: *Locality* 「幅のみ等倍」と「軸別」が同居して正解が型から読めない状態が解消。
  *Tests* 往復変換のプロパティテスト 1 本。`authoring_coords` 13 / `scale.rs` 20 の重複が消える。

### 6. `anaden-studio` の 49 モジュールを機能単位に畳む

- **Files**: `lib.rs:1-49`, バレル 7 本（`app_state` 8 / `app_ui` 2 バイト未参照 /
  `scenario_ui` 6 / `scenario_editor` 7 / `log_view` 6 / `runner` 5 / `tasks` 11）,
  `app_state_core.rs:55-98`, `scenario_*.rs` 6 ファイル
- **Problem**: 49 個すべて `pub mod`。バレルは計 43 行だが実際に経路になっている
  （`scenario_load.rs:5` → `scenario_ui` → `scenario_editor` → `scenario_state`）。
  `StudioApp` は 43 フィールド単一構造体、`impl StudioApp` が 7 ファイルに分散し、
  中に egui の UI オブジェクトが「状態」として入る。`ScenarioEditorState` は
  `scenario_load.rs:49` がリテラル再構築、`impl` は `scenario_validate.rs:41`。
  同一テストフィクスチャが 5/4 ファイルに逐語コピー。
- **Solution**: バレルと空ファイル削除、`authoring/` `tasks/` `scenario/` `runner/`
  `history/` へ再編。各機能が自分の状態構造体を持ち、`StudioApp` は 5 つの合成に。
- **Benefits**: *Locality* シナリオ schema 変更が 1 機能に閉じる。*Leverage* 機能状態は
  小さなインタフェース。*Tests* ゼロテストの `app_ui_authoring.rs`(379) / `canvas.rs`(141) /
  `app_ui_batch.rs`(78) が描画から切り離されて初めてテスト可能になる
  （描画テストは現在 `completes_without_panic` のみ）。
- **Gate 根拠**: `app_ui_authoring.rs` の section 関数が allowlist 19 組。

### 7. GUI が CLI 契約を再実装している

- **Files**: `studio/runner_exec.rs:10-235`, `studio/tasks_queue.rs:403-426`,
  `cli/main.rs:38-77`, `cli/contract.rs`（既に lib crate `anaden_cli_contract`）
- **Problem**: GUI 内に手書き argv 文法（`is_pipeline_positional:79`, `is_bare_pipeline_name:83`,
  `resolve_positional_token:87`, `resolve_pipeline_arg:102`）。`with_report_file_arg:120` が
  `--report-file` を追記し `report_file_from_args:128` が再パース。同じ `run` が clap 定義と
  文字列組み立て 2 経路で定義され、`--algorithm` 上書きループも 2 箇所。
- **Solution**: `anaden_cli_contract` に型付き実行要求とレポート読込を置き、
  `to_args()` / `from_args()` を対で持たせる。studio は spawn と表示だけ。
- **Benefits**: *Leverage* フラグ追加が 1 箇所（現在 clap + 3 ビルダ + report 追記）。
  *Tests* 往復プロパティテストで手書き文法のバグを型で防ぐ。

### 8. 死んだ・名ばかりの継ぎ目を削る

- **Files**: `studio/app_ui.rs`（2 バイト、`lib.rs:6` で宣言・未参照）,
  `pipeline_driver.rs:359` `is_recoverable_io_error`（lib 再輸出、参照は自身のテストのみ）,
  `pipeline_driver.rs:508` `scaler()`（呼び出し 0）, `pipeline_runner.rs:47` `advance_next`
  （呼び出し 1）, `device/ensure.rs` の `NotReady`, `vision/scale.rs:5` `BASE_HEIGHT`,
  `device/win32_capture.rs:267,288`（`luminance_variance`, `fn _unused()`）,
  `anaden-strategies/src/catalog.rs` 全体
- **Problem**: 削除テストに耐えない公開面が残り「何かの継ぎ目だ」と誤読させる。
  `anaden-strategies` は `anaden-core` 非依存（Cargo.toml に依存なし）、実装ゼロ、2 行の
  データ + `to_run_args()` のみ。`docs/wiki.md:136` は「`Strategy` trait を定義」と書くが
  trait は存在しない。
- **Solution**: 死んだ再輸出/アクセサ/空モジュールを削除。`anaden-strategies` は実装を持つ
  カタログに育てるか、`to_run_args` を studio へ畳んで廃止するか決める。
- **Benefits**: *Locality* 探索コスト減。*Leverage* 公開面が使用実態と一致。
  *Tests* 「存在だけを守るテスト」が不要になる。

### 9.（補助）テストダブルを共有する

- **Problem**: `FakeCapture`/`FakeInput` が 3 箇所に別定義。テストは private 関数へ直接到達し
  (`mtime_unchanged` 等)、`pipeline_driver` の env 依存テストは `unsafe { std::env::set_var }`
  と `EnvUnset` ガードを 4 回複製。`README.md` の中身を assert するテストが 2 本。
- **Solution**: crate ごとに `testing` モジュール（フェイク + スクリプト化クロック）を公開し、
  資産回帰テストは `tests/` へ。
- **Benefits**: *Tests* 新規エンジンテストのコストが 60 行→5 行。インタフェースがテスト面になる。

---

## 4. 推奨順序（採否は未決定）

| 順 | 候補 | 理由 |
|---|---|---|
| 1 | **8**（削除） | 依存ゼロ・低リスク。以後の探索コストを下げる |
| 2 | **2**（語彙統合） | 1・3・5・7 が全部この語彙に触る。先に決めると後段が縮む |
| 3 | **1**（`Pipeline`） | locality 最大。14 studio ファイルの波及を断つ |
| 4 | **5**（幾何型） | バグ risk 最大（幅のみ等倍 vs 軸別）。型で曖昧さを消す |
| 5 | **3**（policy 値） | 安価。1 の直後が自然 |
| 6 | **4**（device 継ぎ目） | 効果大だが変更範囲も大。5 の後 |
| 7 | **7**（CLI 契約） | 独立。単独で実施可能 |
| 8 | **6**（studio 再編） | churn 大。機能追加の直前にやるのが効率的 |
| 9 | **9**（テストダブル） | 2 と並行可 |

## 5. 未検証・要確認

- `docs/wiki.md` の記述（`Strategy` trait、`anaden-core` の trait 一覧）は実装と不一致。
  候補 1・8 の実施時に同時修正するのが妥当。
- `docs/goal-manifest.md:44` は `goal.rs` に存在しないモジュール doc を参照している。
- `GoalReport`（`goal.rs:121-128`）の `reason` / `iterations` / `last_match` は生産側で
  破棄されている（`pipeline_driver.rs:1417,1420` が `Reached(_)` / `Failed(_)`）。
  ゴール達成の診断情報を出すならここが起点になる。
- `All[{TemplateMatch A}, {TemplateMatch B}]` は `GoalStatusContext` が 1 スロットのため
  構造的に到達不能（`goal.rs:106-111` が無条件上書き）— 仕様かバグか要確認。

---

## 6. このレビューを取った時点の作業ツリー

`fix/recovery-cause-classification` ブランチ。**未コミット変更あり**（本レビューは
触れていない）:

```
M crates/anaden-cli/src/main.rs               (+61/-?  recovery hook)
M crates/anaden-device/src/error.rs           (+PlacementFault)
M crates/anaden-device/src/lib.rs             (+win32_window 登録)
M crates/anaden-device/src/win32_input.rs
M crates/anaden-device/src/win32_launch.rs
M crates/anaden-engine/src/lib.rs
M crates/anaden-engine/src/pipeline_driver.rs (+76 line)
?? crates/anaden-device/src/win32_window.rs   (115 lines, 新規)
```

計測値はこの状態を含む。候補 4（device の継ぎ目）と候補 5（幾何型）はこの差分の上で
再評価すること。

新規 `win32_window.rs` についての追加観測（候補 5 の補強）:

- 矩形表現が **6 種類目**になった: `ScreenRegion`(u32), `RECT`, `CropInfo`,
  `[u32; 4]`(ROI), `(f32, f32)`(studio ウィジェット), `(i32,i32,i32,i32)`(本ファイル)。
- `clamped_top_left:26-50` は純粋関数（テスト可能）だが `#[cfg(test)]` を持たない。
- 同ファイルは `#![cfg(windows)]` の内部属性を持たず、兄弟の `win32_*.rs` と不揃い
  （`lib.rs` 側の `#[cfg(windows)]` のみで守られている）。
- `DeviceError::CommandFailed` に日本語文言で 4 種類の失敗理由が集約されており、
  候補 4 の「エラー型に構造が無い」観測を裏づける。

---

## 7. 候補 1 の設計比較（Design It Twice, 2026-11-11）

制約 C1–C11 を固定したうえで、4 つの設計を互いに排他的な目的関数で作らせた。

| | 設計 A: 最小 | 設計 B: 最大柔軟 | 設計 C: 呼び出し側 1 行 | 設計 D: ports & adapters |
|---|---|---|---|---|
| 入口 | `Pipeline::{load, step, save, inspect}` | `PipelineBuilder` → `PipelineBundle` → `Pipeline`/`PipelineDocument` | `anaden_cli::live::run(dir, start, opts, ev)` + `Session::run` | `Pipeline` + 3 port (`Templates`/`Matcher`/`PipelineStore`) |
| 実装ファイル | 1 | 15 | 4（vision 2 + engine 1 + cli 1） | 5 |
| trait 数 | 0 | 約 10 | 0 | 3 |
| `TaskDef` | Deserialize のみ（ワイヤ行に純化） | ワイヤ行 + `TaskView` | ワイヤ行 + `Task` | ワイヤ行 + `Task` |
| ドミナント経路の行数 | 変化なし | 変化なし | 281 → 1 文 | 変化なし |

**A（最小）** — 入口 4 つ。`inspect` が `diagnose.rs` の `detect` 再実装を吸収する。
`TaskDef` から `Serialize` を落とすことで、相対化しない第 2 の書き手
(`app_state_pipeline_task.rs:94` の `toml::to_string(spec)`) をコンパイルエラーにする。
削除テスト: `Pipeline` を消すと、各呼び出し側が根の解決・クロップ倍率・アルゴリズム分岐・
キャッシュ鍵を再度書くことになる — 4 つとも 1 呼び出しで得ている。

**B（最大柔軟）** — 将来の retry / timeout / fallback / 複数テンプレート源 / 差し替え
マッチャに継ぎ目を先に用意する。`TemplateSource`(fs + in-memory)、`Matcher`(sse + ccoeff)、
`PipelineWorkspace`(fs + memory) はいずれも 2 アダプタで「実在する継ぎ目」の条件を満たす。
ただし Interface が実装と同じ複雑さになる: `BindingPolicy::{Eager,Lazy}`、
`WatchMode::{Include,Exclude,ManifestOnly}`、
`TemplatePathPolicy::{Preserve,RelativizeToRoot,RejectAbsolute}`、`ValidationMode`、
`Severity` の 7 つの直交ノブを呼び出し側が理解しないと正しく使えない。Depth が
「小さな Interface の背後の振る舞い」ではなく「大きな Interface の背後の選択肢」に変わる。
さらに retry/timeout/fallback の TOML 構文は存在せず、`deny_unknown_fields` のため
どのみちスキーマ変更になる — 継ぎ目だけ先に作っても extension point にはならない。

**C（呼び出し側 1 行）** — `main.rs:325-490` + `pipeline_driver.rs:516-630` の 281 行
（ヘルパ込み 388 行）を 1 文にする。ただし代償は実行ポリシー（`recovery`、
`verify_after_fire`、`jitter`、`cancel`、`ensure_open`）を `SessionOptions` 14 フィールドに
移すことで、これは候補 3 と候補 2 の仕事。`save_task_def` は自由関数のまま残り、
パス二重性に触れないので `Path::new("")` の欠陥は生存する。`Session<C: Capture, I: Input>`
は存在しない device trait（候補 4）を前提にする。宛先は正しいが切符が違う。

**D（ports & adapters）** — `VisionEngine` を「拡張せず迂回する」と明言した点が誠実
（`match_template_prepared` が inherent 専用で `&dyn VisionEngine` から呼べないため）。
3 port とも 2 アダプタ条件を満たす。ただし fs が `Templates` と `PipelineStore` の
両方に現れ、1 つの関心に 2 つの port を置いている。ホットパスに `Arc<dyn Matcher>` の
間接が入る。そして決定的に、**パス解決が `TemplateSource` の関心のまま**なので、
`TaskDef` が相対と絶対を兼ねるという今回の欠陥は port の内側に隠れるだけで消えない。

### 7.1 推奨: A の形 + 3 つの移植、2 つの拒否

採る:

1. **A の `Pipeline::{load, step, save, inspect}`** — 入口 4 つ、trait 0。
2. **D の `TaskDef` / `Task` 分離** — ただし port ではなく private 型として。
   「書かれるもの」と「実行されるもの」を型で分ける。`Task.template` は絶対、
   `TaskDef.template` は著者記載の相対。
3. **C の `Cursor`** — `step(&self, frame, current: &str)` ではなく
   `tick(&self, cursor: &mut Cursor, frame)`。文字列名の受け渡しを 1 箇所減らす。
   これは実行ポリシーを移さないので候補 3 の領域を侵さない。
4. **A の `inspect`** — 閾値 0 の診断を同じ crop + dispatch 経路に乗せ、
   `diagnose.rs:30-125` の複製を消す。

拒む:

- `Arc<dyn Matcher>`。`Algorithm` は TOML スキーマが凍結した 2 値 enum で、
  第 3 の値はスキーマ変更なしには存在し得ない。enum match は継ぎ目ではなく実装。
- `Session`/`live::run` の導入。候補 2・3 の範囲。

### 7.2 移行を決める制約: `assert_tests_untouched`

`scripts/quality/assert_tests_untouched.mjs` は `#[cfg(test)]` 領域と `tests/` 配下の
**差分 1 行**でも exit 1 にする（allowlist 不一致時）。`commit_guard.mjs:73` が commit 時に
これを呼ぶ。したがって「3614 行のテスト塊を継ぎ目で割る」を初回 PR に入れると、
移設した全ファイルを allowlist に載せる必要があり、`_comment` の規約上それは
「機械的でテストの意味を変えない修正」の範囲を超える。

→ **初回 PR はテストを 1 行も触らない。** `load_pipeline` / `save_task_def` /
`run_step` を `Pipeline` の薄いラッパとして残せば、76 の `TaskDef { .. }` リテラルも
3614 行のテストも無変更でコンパイルが通る。テスト分割は第 2 PR。

この制約が設計上の判断を 1 つ強制する: **`run_step(&[TaskDef], ..)` は根を持たない**ので、
初回 PR では `TaskDef.template` の絶対化を `load_pipeline` に残す（現行挙動の保存）。
「決して絶対化しない」は第 2 PR で `run_step` を消すときに初めて成立する。
そうしないと、リポジトリ根以外から起動した実行が壊れる。

### 7.3 未決の設計質問

1. `TaskDef` から `Serialize` を落とすか（第 2 の書き手をコンパイルエラーにする）。
   影響は `app_state_pipeline_task.rs:94` の 1 箇所のみ（テストは `TaskDef` を
   直接 serialize していない — `toml::to_string` は `pipeline.rs:508` と同 1 箇所だけ）。
2. `watch/` を `Pipeline` が持つか、呼び出し側が合成し続けるか。
3. テンプレートキャッシュをプロセス全体 `static` のままにするか、`Pipeline` の
   インスタンスにするか（後者は「ファイル削除後も一致する」テスト 1 本を壊す）。
4. 名前引きを線形走査のままにするか `HashMap` 索引にするか（内部判断でよい）。

---

## 8. 決定: 候補 1 の実装 (2026-11-11)

7.1 の推奨をそのまま採った。ブランチ `refactor/vision-pipeline-module`。

### 8.1 決定事項

| 論点 | 決定 | 理由 |
|---|---|---|
| `TaskDef` の `Serialize` | 落とす | 相対化しない第 2 の書き手 (`app_state_pipeline_task.rs:94`) がコンパイルエラーになり、`save_task_def` に統合された |
| 書き込み型 | `TaskRow`（private, `Serialize` のみ） | フィールド順・属性を `TaskDef` と一致させ、TOML 出力はバイト同一 |
| `watch/` の所有 | `Pipeline` が持つ | 呼び出し側は `watch` サブディレクトリを知らない |
| テンプレートキャッシュ | プロセス全体 `static` のまま | インスタンス化は「ファイル削除後も一致する」テスト 1 本を壊す。別変更 |
| `Cursor` | 見送り | 唯一の利用者が候補 3 の実行ループ。今入れると生産コードから呼ばれない死んだ API になる |
| `Pipeline::save` | 見送り | studio の stem マッピング（既存タスクのファイル名が name と異なる）を扱う必要があり、今の `save_task_def` を置き換えると別の書き手を作る。書き込みの depth は `TaskRow` の強制で達成済み |
| `Algorithm` の trait 化 | 却下 | TOML スキーマが凍結した 2 値 enum。第 3 の値はスキーマ変更なしに存在しない |

### 8.2 構造

```
crates/anaden-vision/src/pipeline.rs        facade + run_step + 既存テスト（無変更）
crates/anaden-vision/src/pipeline/
  schema.rs     TaskDef(Deserialize) / TaskRow(Serialize) / Action / Algorithm / manifest
  task.rs       Task（テンプレート解決済みの実行単位）
  cache.rs      準備済みテンプレートの static キャッシュ
  recognize.rs  照合の唯一の実装（ROI 正規化・クロップ・アルゴリズム分岐・領域オフセット）
  io.rs         読み書き（相対→絶対の解決はここだけ、絶対→相対は save_task_def だけ）
  runner.rs     Pipeline（load/step/inspect）+ StepOutcome + 診断行の生成
```

`Pipeline` の Interface:

- `load(dir)` — チェーン + `watch/` + マニフェストを 1 回で読む
- `step(screenshot, current)` — 名前で引き、照合してアクションと次タスクを返す
- `inspect(screenshot)` — 閾値 0 で全タスクを診断（`diagnose.rs` の重複実装を削除）
- `task(name)` / `tasks()` / `watch_tasks()` / `watch_names()` / `start_task()`
- `from_defs(root, defs)` — I/O なしの組み立て（`run_step` 互換ラッパが使う）

### 8.3 テスト改変ゼロの担保

`assert_tests_untouched.mjs` の制約により、既存テストは 1 行も変えていない。
新しい継ぎ目の検証は新規ファイル `crates/anaden-vision/tests/pipeline_module.rs`
（`tests/` の新規ファイルは `oldLines` が空なので gate の対象外）に置いた。

`load_pipeline` / `save_task_def` / `run_step` は薄いラッパとして残し、76 の
`TaskDef { .. }` リテラルと 3614 行のテストが無変更でコンパイルされるようにした。

### 8.4 既知の限界（正直な申告）

`Pipeline::step` / `Pipeline::inspect` は現時点で**生産コードから呼ばれていない**。
生産経路は従来どおり `run_step`（`anaden-engine/src/pipeline_driver.rs:612,700`）と
`diagnose_all`（`anaden-engine/src/diagnostics.rs:40`）で、どちらも `&[TaskDef]` を
受け取る。driver が `Vec<TaskDef>` を保持している都合上、ここを `Pipeline` に
差し替えると driver のテスト（`TaskDef` リテラルで組み立てている）に波及する。

したがって今回の depth の実体は次の 3 点に限られる。

1. 照合実装が `recognize::probe_template` の 1 本になり、`diagnose_task` の重複
   detect が消えた（`Pipeline::inspect` と `run_step` が同じ関数を通る）
2. 書き込みが `save_task_def` の 1 経路になり、相対化を忘れた第 2 の書き手が
   型（`TaskDef` の `Serialize` 削除）でコンパイル不能になった
3. `watch/` の場所とマニフェストの読み込みが `Pipeline::load` に閉じた

`step` / `inspect` が生産経路に乗るのは候補 3（実行ループの `Cursor` 化）で
driver が `Pipeline` を保持してからになる。それまでは「Interface は用意済み・
呼び出し側の移行待ち」という状態で、削除テストは現状 `recognize.rs` と
`io.rs` に対してのみ通る。
