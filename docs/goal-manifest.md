# Pipeline Manifest (`pipeline.toml`) — Goal-Driven Automation Schema

パイプラインディレクトリのルートに置かれた `pipeline.toml` は、**TaskDef（認識タスク）とは分離された宣言層**で、開始タスクとゴール（終端条件）を宣言する。`load_pipeline` はこのファイルを TaskDef として誤パースせずスキップし、`load_pipeline_manifest` が `start_task` + `goals` に分離パースする。

> 設計背景: Issue #37（ゴール駆動自動化）。TaskDef は「認識とアクション」を宣言し、manifest は「どこから始まり、いつ終わるか」を宣言する。両者を1ファイルに混ぜると `deny_unknown_fields` が効かなくなるため、慣例ファイル名 `pipeline.toml` で物理分離する。

## Schema

```toml
start_task = "<task-name>"          # 必須: 最初に実行する TaskDef 名

[[goal]]                            # 省略可（無宣言 = 無限ループ・後方互換）
name = "<goal-name>"
[goal.stop]
LoopCount = { target = 50 }        # UC-1: N 回反復で停止
# または
# [goal.stop.TemplateMatch]        # UC-2: テンプレートマッチで停止
# task = "clear_button"
# confidence = 0.85
# または
# Timeout = { secs = 3600 }        # UC-3: 指定秒数経過で停止
```

- `start_task`: **必須**。パイプラインディレクトリ内の `*.toml`（TaskDef）の `name` と一致すること。
- `[[goal]]`: 配列（0個以上）。省略時は `goals = []` となり、driver は従来通り**無限ループ**として振る舞う（後方互換）。
- **`deny_unknown_fields`**: 未知フィールドは即時エラー（typo 回帰防止）。`Goal` / `StopCondition` / `PipelineManifest` の全階層で有効。

## StopCondition の3バリアント

各 `goal.stop` は `StopCondition` enum のいずれか1つ。評価セマンティクスの詳細は `anaden-core/src/goal.rs` のモジュール doc 参照（本ドキュメントは TOML 表記の転記が責務）。

### UC-1: `LoopCount`（指定回数反復で停止）

```toml
[[goal]]
name = "farm50"
[goal.stop]
LoopCount = { target = 50 }
```

- `target`: 正の整数（`u64`）。`0` はバリデーションエラー（`GoalError::NonPositive`）。
- セマンティクス: `target` は **evaluate() の呼出回数（tick 数）**。認識 NoMatch・アクションエラーを含む全反復を1 tick として数える（認識成功率に依存しない終端保証）。

### UC-2: `TemplateMatch`（テンプレートマッチで停止）

```toml
[[goal]]
name = "find_clear"
[goal.stop.TemplateMatch]
task = "clear_button"
confidence = 0.85
```

- `task`: マッチ対象のタスク名（テンプレート識別子）。別ディレクトリのテンプレートも参照可。
- `confidence`: マッチ判定の信頼度閾値。`0.0 < confidence <= 1.0`。範囲外は `GoalError::InvalidConfidence`。
- セマンティクス: 直近のマッチ（`last_match`）が `task` と同名かつ `confidence` 以上の信頼度のとき `Reached`。

> 注: UC-2 の実環境動作には、`StepOutcome` が `matched_confidence` を伝播し、driver が `GoalStatusContext.last_match` を構築することが必要（T4 wiring）。manifest の構文解析は本ドキュメントの範囲。

### UC-3: `Timeout`（指定秒数経過で停止）

```toml
[[goal]]
name = "one_hour_limit"
[goal.stop]
Timeout = { secs = 3600 }
```

- `secs`: 正の整数（`u64`）。`0` はバリデーションエラー。
- セマンティクス: `elapsed_secs >= secs` のとき `Failed`（異常終端・exit code 非0）。driver は `Instant::now` または注入された Clock で `elapsed_secs` を計測する。

## TOML 表記のバリエーション

`StopCondition` は内部タグ付き enum 相当（`[goal.stop]` 配下でバリアント名をキーにする）。インライン table とサブテーブル表記の両方が使える:

```toml
# インライン（単純なバリアント）
[goal.stop]
LoopCount = { target = 50 }
Timeout = { secs = 3600 }

# サブテーブル（フィールド付きバリアント）
[goal.stop.TemplateMatch]
task = "clear_button"
confidence = 0.85
```

## エラー処理

`load_pipeline_manifest` は以下の場合に `TaskDefError::ParseFailed` を返す:

| ケース | 理由 |
|---|---|
| `pipeline.toml` 不在 | manifest 経路を選んだ呼出側は存在を期待するため即時 fail（`load_pipeline` の「空 Vec」緩契約とは意図的に異なる） |
| TOML 構文エラー | `toml::from_str` 失敗 |
| `start_task` 欠落 | 必須フィールド |
| 未知フィールド（top/goal/stop 各階層） | `deny_unknown_fields` 違反 |
| `confidence` 範囲外 / `target` / `secs` が 0 | バリデーションエラー（※現状はパース時には検証せず、driver 層で `Goal::validate()` を呼ぶ設計） |

## 実例

`templates/pipelines/field_loop_pc/pipeline.toml` に UC-1 (LoopCount) の実例を置いている。TaskDef（`tap_bottom.toml` / `tap_hud_tr.toml`）と同じディレクトリに共存し、`load_pipeline` は manifest をスキップして TaskDef のみを、`load_pipeline_manifest` は manifest のみを読み込む（分離契約の回帰テスト: `load_pipeline_and_load_pipeline_manifest_are_separated`）。

## 後方互換（no-goal = 無限ループ）

`[[goal]]` 宣言が無い manifest は `goals: []` として読み込まれる。driver は `goals` が空のとき `Goal` 無し（= 従来の無限ループ）として振る舞うため、既存パイプラインに manifest を新設しても挙動は変わらない（acceptance criterion: no-goal 宣言時の後方互換）。
