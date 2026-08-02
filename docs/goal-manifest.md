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

## StopCondition のバリアント

各 `goal.stop` は `StopCondition` enum のいずれか1つ。基本3バリアント（`LoopCount` / `TemplateMatch` / `Timeout`）に加え、複数条件の AND/OR を表現する合成バリアント（`All` / `Any`）が存在する。評価セマンティクスの詳細は `anaden-core/src/goal.rs` のモジュール doc 参照（本ドキュメントは TOML 表記の転記が責務）。

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

## StopCondition 合成バリアント（All / Any）

単一バリアントでは表現できない「複数条件の AND/OR」を表現するため、`StopCondition` は再帰的な合成バリアント `All` / `Any` を持つ（Issue #50）。どちらも `conditions: Vec<StopCondition>` を持ち、Vec 経由の間接再帰により `Box` 不要（Rust 標準パターン）。

- `All { conditions }`: **AND 合成**。子条件が**全て** `Reached` のとき `Reached`。子のいずれかが `Failed` なら `Failed`（異常終端を優先）。
- `Any { conditions }`: **OR 合成**。子条件の**いずれか**が `Reached` のとき `Reached`。子のいずれかが `Failed` なら `Failed` を優先（`Reached` より `Failed` を先に判定し、異常終端を逃さない）。

> 子条件の評価で `Failed` と `Reached` が両方存在する場合、**`Failed` を優先**する（`Any` でも `Reached` より `Failed` が勝つ）。これは driver の exit code 分岐（`EXIT_RUN_TIMEOUT = 2`）に直結する意味論決定点で、`anaden-core/src/goal.rs` の `evaluate` 実装とテストで固定される。

### 再帰合成

`conditions` の要素もまた `All` / `Any` になれる（再帰合成可）。これにより `(A AND B) OR C` のような任意のブール木を表現できる。深さに制限は設けないが、実用上は 2〜3 階層程度にとどめることが推奨される（可読性・デバッグ性）。

### UC-AND（All）: 複数条件の AND

「50回反復 **かつ** クリアボタンが出現したら停止」の例:

```toml
[[goal]]
name = "farm50_and_clear"
[goal.stop.All]
conditions = [
  { LoopCount = { target = 50 } },
  { TemplateMatch = { task = "clear_button", confidence = 0.85 } },
]
```

サブテーブル表記（`[[goal.stop.All.conditions]]`）を使って子条件を1つずつ宣言することも可能（後述「TOML 表記のバリエーション」参照）:

```toml
[[goal]]
name = "farm50_and_clear_subtable"
[goal.stop.All]
[[goal.stop.All.conditions]]
LoopCount = { target = 50 }
[[goal.stop.All.conditions]]
[goal.stop.All.conditions.TemplateMatch]
task = "clear_button"
confidence = 0.85
```

### UC-OR（Any）: 複数条件の OR

「クリアボタンが出現 **または** 1時間経過したら停止」の例（いずれか成立で終端）:

```toml
[[goal]]
name = "clear_or_one_hour"
[goal.stop.Any]
conditions = [
  { TemplateMatch = { task = "clear_button", confidence = 0.85 } },
  { Timeout = { secs = 3600 } },
]
```

> `Timeout` が `Failed`（異常終端）を返す点に注意。`Any` 合成では `Failed` が `Reached` より優先されるため、`TemplateMatch` 到達と `Timeout` 失敗が同 tick で成立した場合は `Failed` となり exit code 非0で終了する（異常を正常で上書きしない）。

### 空 `conditions` は拒否される

`All` / `Any` いずれも `conditions` が空 Vec（`conditions = []`）の場合はバリデーションエラーとなる。合成の最小単位は子条件1つ以上（`Goal::validate` が `validate` 再帰で検査）。空合成は「絶対に到達しない（All）」「即時到達（Any）」という自明で紛らわしい意味論を生むため、宣言時点で弾く。

### 非スコープ: `Not`（否定合成）

否定 `Not { condition }` バリアントは**本機能のスコープ外**とする（Issue #50）。理由は (1) `Timeout` の `Not` が「タイムアウトするまで継続」を意味し終端保証を損なう、(2) 実用上のユースケースが限られる、(3) 評価の意味論（特に `Failed` の否定）が複雑化する。必要になった場合は別 Issue で検証セマンティクスと共に設計する。

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

合成バリアント（`All` / `Any`）は `conditions` 配列を持つため、**配列内のインライン table** と **array-of-tables（`[[...conditions]]`）** の2表記が使える（どちらも serde が同じ `Vec<StopCondition>` へデシリアライズする）:

```toml
# (1) インライン table の配列（簡潔・1ファイル読み向け）
[goal.stop.Any]
conditions = [
  { TemplateMatch = { task = "clear_button", confidence = 0.85 } },
  { Timeout = { secs = 3600 } },
]

# (2) array-of-tables（子条件ごとに宣言・ネストが深い場合に可読性向上）
[goal.stop.All]
[[goal.stop.All.conditions]]
LoopCount = { target = 50 }
[[goal.stop.All.conditions]]
[goal.stop.All.conditions.TemplateMatch]
task = "clear_button"
confidence = 0.85
```

> `deny_unknown_fields` は合成バリアントの**全階層**で有効。`[goal.stop.All]` 配下の未知フィールド、`conditions` 内の子バリアントの未知フィールドも即時エラーとなる（ネスト round-trip の回帰テストで検証: `anaden-core/src/goal.rs` の `deserialize_all_inline` / `deserialize_any_subtable` 系）。

## エラー処理

`load_pipeline_manifest` は以下の場合に `TaskDefError::ParseFailed` を返す:

| ケース | 理由 |
|---|---|
| `pipeline.toml` 不在 | manifest 経路を選んだ呼出側は存在を期待するため即時 fail（`load_pipeline` の「空 Vec」緩契約とは意図的に異なる） |
| TOML 構文エラー | `toml::from_str` 失敗 |
| `start_task` 欠落 | 必須フィールド |
| 未知フィールド（top/goal/stop/合成子 各階層） | `deny_unknown_fields` 違反（合成バリアントのネスト先も含む） |
| `confidence` 範囲外 / `target` / `secs` が 0 | バリデーションエラー（※現状はパース時には検証せず、driver 層で `Goal::validate()` を呼ぶ設計） |
| `All` / `Any` の `conditions` が空 | バリデーションエラー（`validate` 再帰検査。※driver 層の `Goal::validate()` で検出） |

## 実例

`templates/pipelines/field_loop_pc/pipeline.toml` に UC-1 (LoopCount) の実例を置いている。TaskDef（`tap_bottom.toml` / `tap_hud_tr.toml`）と同じディレクトリに共存し、`load_pipeline` は manifest をスキップして TaskDef のみを、`load_pipeline_manifest` は manifest のみを読み込む（分離契約の回帰テスト: `load_pipeline_and_load_pipeline_manifest_are_separated`）。

## 後方互換（no-goal = 無限ループ）

`[[goal]]` 宣言が無い manifest は `goals: []` として読み込まれる。driver は `goals` が空のとき `Goal` 無し（= 従来の無限ループ）として振る舞うため、既存パイプラインに manifest を新設しても挙動は変わらない（acceptance criterion: no-goal 宣言時の後方互換）。
