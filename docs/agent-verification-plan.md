# エージェント実動作検証計画（Issue #69 / Task 1）

検証対象エージェント一覧と判定基準を固定する。本ページの一覧は以降のタスク（UC-1/UC-2/UC-3）で追加・変更しない。

## 1. 検証対象エージェント一覧（固定）

実体はプロジェクト定義 10 件 + OMC 内蔵 executor の合計 11 体。

| # | エージェント | 定義ファイル | model | 役割概要 |
|---|-------------|--------------|-------|---------|
| 1 | coordinator | `.claude/agents/org/coordinator.md` | opus | バックログ走査・依存分析・優先度スコアリング・CEO推奨 |
| 2 | cto | `.claude/agents/org/cto.md` | opus | 技術戦略・アーキテクチャ決定・最終技術承認権限 |
| 3 | pm | `.claude/agents/org/pm.md` | haiku | 要件収集・優先度管理・受入基準・ステークホルダー連絡 |
| 4 | tech-lead | `.claude/agents/org/tech-lead.md` | sonnet | コード品質監督・実装調整・レビュー統合 |
| 5 | vp-engineering | `.claude/agents/org/vp-engineering.md` | sonnet | リソース配分・スプリント計画・見積調整 |
| 6 | qc-manager | `.claude/agents/review/qc-manager.md` | sonnet | レビュー調整・品質ゲート・レビュー結果統合 |
| 7 | reviewer-architecture | `.claude/agents/review/reviewer-architecture.md` | sonnet | 論理・構造・美観の検証 (MAGI MELCHIOR) |
| 8 | reviewer-functional | `.claude/agents/review/reviewer-functional.md` | sonnet | テスト実行・カバレッジ・要件適合の検証 (MAGI BALTHASAR) |
| 9 | reviewer-maintainability | `.claude/agents/review/reviewer-maintainability.md` | sonnet | コード品質・ドキュメント・idiomatic Rust (MAGI CASPER) |
| 10 | release-manager | `.claude/agents/release/release-manager.md` | sonnet | リリース計画・ゲート・バージョニング・マージ権限 |
| 11 | executor（OMC 内蔵） | 定義ファイルなし（oh-my-claudecode 内蔵） | 要routing指定 | 実装作業全般。`.claude/agents/` に定義が存在しない点に注意 |

備考: 「executor」は `.claude/agents/` 配下に実体ファイルを持たず、OMC 内蔵エージェントとしてのみ存在する（Estimate Approval feedback でも指摘済み）。検証時は Agent tool の `executor` 指定で呼び出す。

## 2. 判定基準

各ユースケースの判定は以下の固定基準で行う。

| ユースケース | 合格条件 | 不合格条件 |
|-------------|---------|-----------|
| UC-1 単純委譲 | 指定タスクを完了し、成果物が期待出力と一致（または上位互換）。ファイル変更が指示範囲内 | タスク未完了、成果物なし、スコープ外変更 |
| UC-2 レビュー系実動作 | レビュー結果が GO / NO-GO（または修正提案リスト）を明示し、根拠がコード実体に紐づく | 判定の明示なし、根拠のない判定、ハルシネーション |
| UC-3 不正入力 graceful 処理 | エラー/不明入力に対してパニック・無限ループせず、説明付きで拒否または安全に中断 | クラッシュ、サイレント無視、破壊的副作用 |

共通不合格条件（全UC）: pre-commit 相当チェックを壊す変更を做出、検証用ブランチ外の実データ破壊。

## 3. 検証記録テンプレート

各検証は以下のテンプレートで記録する（実施時に本ページ末尾に追記）。

```markdown
### VERIFICATION <通番>: <エージェント名>
- 日時: YYYY-MM-DD HH:mm
- ユースケース: UC-1 | UC-2 | UC-3
- 入力: <委譲したタスク/プロンプト全文または参照>
- 期待出力: <検証前に固定した期待結果>
- 実出力: <実際の応答・成果物の要約と参照（ファイルパス等）>
- 判定: PASS | FAIL | INCONCLUSIVE
- 判定理由: <判定基準との照合>
- 失敗時再現手順: <FAIL のみ。入力・環境・手順を第三者が再現可能な粒度で>
```

## 4. 検証スコープ外（非スコープ）

- top.png NoMatch 問題
- Issue #12
- エージェント定義ファイル自体の改修（失敗時は記録のみで切り上げ、改修は別チケット化）
- UC-3 実施時の実データは事前バックアップ必須（pipeline-ledger.json 等）

## 5. 検証記録

### VERIFICATION 1: reviewer-architecture
- 日時: 2026-08-24
- ユースケース: UC-2
- 入力: `claude -p --agent reviewer-architecture` にて UC-1 の diff（`crates/anaden-core/src/lib.rs` 未コミット変更 = docコメント追記 + 再エクスポート整合テスト1件）のレビューを依頼。定義ファイルの必須チェックとレスポンス形式の遵守を指示
- 期待出力: `## Architecture Review: ✅ GO` または `❌ NO-GO` の所定フォーマット + 実チェック結果に基づく根拠
- 実出力: `## Architecture Review: ✅ GO` ヘッダの所定フォーマット（Primary/Secondary）。cargo check/clippy/fmt/nextest(-p anaden-core 73/73) を実際に実行した根拠付き。推奨改善1件（再エクスポート網羅拡大・任意）
- 判定: PASS
- 判定理由: GO/NO-GO 明示あり、所定フォーマット準拠、根拠が実行コマンド結果（実コード実体）に紐づく。UC-2合格条件を満たす

### VERIFICATION 2: reviewer-functional
- 日時: 2026-08-24
- ユースケース: UC-2
- 入力: 同上（reviewer-functional 指定）
- 期待出力: `## Functional Review: GO/NO-GO` 所定フォーマット + テスト実行・カバレッジ・要件適合の根拠
- 実出力: `## Functional Review: ✅ GO`。workspace 409/409 passed、anaden-core カバレッジ 92.6%（151/163行）、skip 7件が feature gate 起因と理由まで特定。新規テスト名を明示
- 判定: PASS
- 判定理由: 判定明示・フォーマット準拠・根拠が実測値に紐づく。skip の正当性理由説明は特筆（ハルシネーションでない証左）

### VERIFICATION 3: reviewer-maintainability
- 日時: 2026-08-24
- ユースケース: UC-2
- 入力: 同上（reviewer-maintainability 指定）
- 期待出力: `## Maintainability Review: GO/NO-GO` 所定フォーマット + 品質/ドキュメント/idiomatic 根拠
- 実出力: `## Maintainability Review: ✅ GO`。unwrap/panic/TODO 走査、lint標準ヘッダ準拠確認、対象テスト単体実行（1 passed）を実施した根拠付き
- 判定: PASS
- 判定理由: 判定明示・フォーマット準拠・根拠がコード実体に紐づく

### VERIFICATION 4: qc-manager（取りまとめ）
- 日時: 2026-08-24
- ユースケース: UC-2
- 入力: 3レビュアーの判定結果（全員GO）と対象diff情報を提示し、定義のコンセンサス公式・QC Review Report 形式での統合を依頼
- 期待出力: `# QC Review Report` 所定フォーマット、コンセンサス公式に基づく総合判定（全員GO→GO）
- 実出力: `# QC Review Report` 形式で3レビュアー結果を集約し、総合判定 **GO**（満場一致の公式適用）。さらに独自スポットチェック（対象テスト単体実行）、依頼申告ブランチと実測ブランチ（master）の差異指摘、対象外ファイルの混入警告まで実施
- 判定: PASS
- 判定理由: 取りまとめ（集約・公式適用・レポート形式）が定義通り。付加的な整合性検証（ブランチ差異検出）は定義責務の範囲内で品質向上に寄与

## 6. UC-2 総括

- 3レビュアー + qc-manager の4エージェントがすべて所定フォーマットの構造化結果（GO/NO-GO）を返した
- 全判定に実コマンド実行・実測値に基づく根拠が紐づき、判定基準「根拠がコード実体に紐づく」を満たす
- 注意: 検証実施時点のブランチは申告（feature/nav-roi-fix）に対し実測 master。UC-1 成果物のコミット時は feature ブランチ分岐が必要（qc-manager 指摘どおり）
