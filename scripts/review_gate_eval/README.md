# review-gate-eval — メトリクス集計 (issue #76 Task 3)

Task 1 の golden dataset（既知問題の正解リスト）と Task 2 の review-gate 実行結果
（3レビュアーの指摘 + QC Manager コンセンサス判定）から、PR ごと / 全体の

1. **recall** — 既知問題の検出再現性（正解は PR×golden-id 単位で一意に数える）
2. **偽陽性率 (fp_rate)** — 正解に対応しない指摘の割合。**必ず N（total_findings）と
   Wilson 95% 信頼区間とともに報告**する。分母 0 は点推定せず null
3. **コンセンサス妥当性 (consensus)** — GO/NO-GO 判定と「マージ後に問題が判明したか」
   の混同行列（tp/fp/tn/false_negatives + accuracy）。NO-GO なのにマージされた PR も列挙

を集計する。workspace には含めない独立パッケージ（crate 依存・結合原則への影響ゼロ）。

## golden issue 抽出基準（Task 1 の客観的ルール、estimate 承認条件より）

PR 本文・PR comments から以下を満たすものを golden とする:

- PR 本文の課題記述や fix コミットメッセージが「修正対象の問題」として明示されている
- レビュー comments で指摘され、コミットで実際に修正が入ったもの
- マージ後に issue/comment で問題と確認されたもの（→ `post_merge_issue_ids`）

主観的な改善提案（スタイル好み等）は golden に含めない。

## 入力スキーマ（input.json）

```json
{
  "golden_issues": [
    {"pr": 58, "id": "g58-1", "description": "ROIが実測と乖離"}
  ],
  "findings": [
    {"pr": 58, "reviewer": "architecture", "matched_golden": "g58-1",
     "confidence": 0.9, "command_results": {"clippy": "PASS", "nextest": "PASS"}},
    {"pr": 58, "reviewer": "functional", "matched_golden": null,
     "confidence": 0.3, "command_results": {"clippy": "PASS", "nextest": "FAIL"}}
  ],
  "consensus": [
    {"pr": 58, "verdict": "GO", "merged": true, "post_merge_issue_ids": [],
     "split_info": {
       "judgments": [
         {"reviewer": "architecture", "verdict": "GO", "confidence": 0.9, "has_critical": false},
         {"reviewer": "functional",   "verdict": "GO", "confidence": 0.85, "has_critical": false},
         {"reviewer": "maintainability", "verdict": "NO_GO", "confidence": 0.3, "has_critical": false}
       ],
       "veto_activated": false,
       "command_fail_forced_nogo": false
     }}
  ]
}
```

- `matched_golden`: 評価者が指摘を正解問題と対応づけられる場合のみ golden id を設定。
  dataset に存在しない id を参照すると集計エラー（exit 1）
- `verdict`: `"GO"` / `"NO_GO"`
- `confidence` (#80): レビュアーの自己申告信頼度 0.0-1.0。**旧データ互換で `#[serde(default)]`**
  （欠損は `null`）
- `command_results` (#80): clippy / nextest の決定論的成否（`"PASS"` / `"FAIL"`）。
  欠損（`null`、旧データ・未実施）と失敗（`FAIL`）は区別され、**欠損が暗黙に成功扱いにならない**。
  いずれかの明示的 `FAIL` は veto と同格の強制 NO-GO（`split_info.command_fail_forced_nogo`）
- `split_info` (#80): コンセンサスの割れ記録。`aggregate()` は `split_info` が存在する場合
  `effective_verdict()`（決着方式: `COMMAND_FAIL` > `CRITICAL_VETO` > `MAJORITY`/`UNANIMOUS_GO`）
  を優先し、欠損レコードは従来どおり `verdict` フィールドで集計する（旧 results.json 再集計互換）
- 非決定性の揺らぎ（同一 PR の再実行差分）は、findings を実行回でエントリ倍加させて
  N を増やした上で Wilson CI で報告する

## 実行

```bash
cargo run --release -- input.json            # メトリクス JSON を stdout
cargo test                                   # 14 tests
cargo clippy --all-targets -- -D warnings
```

## クリーンアップ

評価に用いた worktree/一時ブランチは評価完了後に削除すること（master へ影響を残さない）。

---

## 評価結果 (2026-08-25 実施, issue #77 クローズ時点)

入力: `data/input.json`（golden 16 issues / 6 PR + 実測代替レーンの findings + コンセンサス記録 9 PR）。
生データ・抽出基準の詳細: `.omc/research/review-gate-eval/golden-issues.md`（G1–G4 基準）と
`docs/anaden-helper.wiki/review-gate-validation.md`。

### 数値

| 指標 | 値 |
|------|-----|
| recall | 16/16 = 1.00 — ただし下記のとおり **upper-bound (構造分析レーン込み)** |
| fp_rate | 17/33 = 0.515, **Wilson 95% CI [0.352, 0.675]** (N=33) |
| コンセンサス混同行列 | TP=3 (PR #64/#65 empty-commit 阻止, #75 チケット取り違え捕捉) / FP=0 / TN=6 / FN=0, accuracy 9/9 |
| NO-GO マージ済み | PR #75 (取り違えは手動修正の上マージ — プロセス逸脱として列挙) |

### この数値が upper-bound である理由（必読）

recall=1.00 は**実測のみの結果ではない**。findings は 3 レーンの合成である:

1. **structure-reproduction-check** — `run_review_gate_eval.sh` の worktree + `git reset --soft`
   による staged-diff 再現が PR 変更内容を機械的に復元できることを確認するレーン
   （LINECOUNT_MATCH 検証）。diff 由来の問題は漏れなく「検出」できるため recall に寄与するが、
   これは review-gate の能力ではなく**再現基盤の能力**
2. **past-gate-evidence** — 過去の commit-gate / Release Review 実績（PR 本文の NO-GO 対応記録等）
   の事後検証レーン。golden の由来（G1/G2）と同一ソースを含むため recall に寄与するが循環性がある
3. **llm-reviewer-sample** — LLM reviewer による単発サンプル（PR #62 のみ, N=4）。非決定性を
   制御できないため点推定としては報告しない

真の review-gate 実測 recall はレーン 3 のみで測るべきだが、golden 分母が小さく
（16 issues / 6 PR）、単発 LLM 実行は非決定的なため、実用的な検出力がない。
よって **recall=1.00 は upper-bound であり実効値ではない**。

### 構造分析の限界（fp_rate の解釈）

fp_rate 0.515 (CI [0.352, 0.675]) は以下の構造的歪みを含む:

- **過大評価要因 (#79)**: review-gate の findings に重複除去・正規化機構がなく、
  3 reviewer の同一指摘が複数カウントされる。本評価の fp レーンにも同様の重複が混入し得る
- **帰結の非対称 (#80)**: 完全 AND コンセンサスのため偽ブロック率が 1-(1-p)^3 に増幅される
  (p=0.1 で 27%)。かつ判定割れの記録が workflow スキーマに存在しないため混同行列の
  FP/FN は過小観測の可能性がある
- **diff 未注入 (#78)**: Analyze phase が diff 本体を reviewer に渡さないため、
  「指摘が PR diff 由来である」保証がなく FP 分類自体が評価者の判断に依存する
  （抽出者バイアス — estimate 承認条件どおり limitations として明記）

### 結論

- fp_rate の Wilson CI 幅（約 ±0.16）はサンプルサイズ N=33 に対して広く、
  定量結論の強さは弱い。gate 調整の根拠にはしない
- コンセンサス実績（empty-commit 3 回阻止・チケット取り違え捕捉・マージ後回帰ゼロ）は
  実績ベースの有効性証拠として残る
- 定量精度の実測は #78 → #79 → #80 改修後の反復実行（Jaccard 類似度主指標）で再測定する
  （#77 方法論）

### 再実行

```bash
cargo run --release -- data/input.json   # data/metrics.json を再生成
```

---

## #80 再検証パス (2026-08-25, Task 5)

スキーマ拡張（confidence / command_results / split_info）とコンセンサス変更
（AND → majority + critical-only-veto + command-fail 強制 NO-GO）適用後の再検証。

### 1. 後方互換の再集計（承認条件）

旧 `data/results.json`（新フィールド欠損）が serde default によりそのまま再集計可能であることを
`results_json_tests.rs::legacy_results_json_reaggregates_via_serde_defaults` で実証。
再生成した `data/metrics.json` は旧値と不変（fp=0, tp=3, tn=6）— split_info 欠損レコードは
legacy `verdict` フィールドで集計されるため。

### 2. 偽ブロック率の構造的低下（混同行列 + golden dataset）

reviewer 1名あたりの偽 NO-GO 確率を p としたとき:

| コンセンサス | 偽ブロック率 (理論) | p=0.1 |
|---|---|---|
| 3 reviewer 完全 AND (旧) | 1-(1-p)^3 | 27.1% |
| majority 2/3 GO (新) | 3p^2(1-p)+p^3 | 2.8% |

- 数式の固定: `metrics_tests.rs::majority_consensus_structurally_reduces_false_block_rate`
  （p ∈ {0.05..0.5} で常に majority < AND）
- **実測 golden dataset 上の Monte-Carlo**（`data/input.json` の 6 clean PR × 3 reviewer、
  p=0.1、2000 trials）: AND = **0.2681**、majority = **0.0294**（理論値 0.271 / 0.028 と整合）。
  偽ブロック率が約 1/9 に構造的に低下することを確認
- 見逃し側の回帰なし: critical finding（`has_critical`）とコマンド失敗
  （`command_fail_forced_nogo`）は majority 判定と二重カウントされず最優先で強制 NO-GO
  （`metrics_tests.rs::critical_veto_split_info_forces_nogo_even_with_majority_go` /
  `command_fail_split_info_forces_nogo_in_aggregation`）
- `data/results.json` の `meta.revalidation_n80` に上記の全記録を埋め込み

### 3. テスト

`cargo test` — metrics 23 / results_json 7 / golden_dataset 10 ほか、全 green。
