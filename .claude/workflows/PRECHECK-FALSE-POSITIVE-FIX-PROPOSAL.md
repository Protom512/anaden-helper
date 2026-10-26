# Ticket-Precheck TOCTOU False-Positive + agent({schema}) 失敗 — 修正提案 (文書化のみ / 適用は CEO 承認経由)

対象: `.claude/workflows/feature-pipeline.js` (ticket-precheck wiring),
`agent({schema})` StructuredOutput 呼出
出所: Issue #220 — feature-pipeline 実行ログ wf_99ca91ef / wf_2000f26f /
wf_15f880f3 ほか計 4 回の同一署名失敗 (org-feedback 2026-10-21 エントリ)
作成: 2026-10-26

## 背景・症状

### FP-A: continuation チケット dispatch 時の TOCTOU race (最重要)

**症状** (4 回観測、各回 ~33-54 万 subagent token を waste):
- PM/coordinator がチケット作成時に `ticket.files` を当時の working-tree diff
  から機械導出する
- その後 (estimate → approve → dispatch の数分間) に 直接エディタ
  (人間/並行 Claude セッション) がコミット・新規ファイルを作る
- dispatch 前の precheck は **実行時点**の diff と比較するため、後発の変更が
  `undeclared changed files` として FAIL → チケットは再起票される
  (waste ループ。#217→#219→#220 の連鎖はこの産物)
- pre-implementation 判定自体 (feature-pipeline.js:505-536) は「declared but
  unchanged は正常・fail は undeclared のみ」で正しい — 問題は **undeclared
  の意味論**。チケット作成後に現れた第三者変更まで「宣言漏れ」と断定している

**修正案 A-1 (basis snapshot)**: チケット作成時に `git write-tree` 相当の
treeHash + `git status --porcelain` を ticket JSON に埋め込む。precheck は
`basis..worktree` の差分のみを undeclared 判定に使い、basis 以前の差分は
チケット作成時点で既知だったものとして扱わない。

**修正案 A-2 (foreign-change defer)**: undeclared ファイルの mtime/コミット
時刻がチケット作成時刻より後なら、FAIL ではなく **defer** (status:
`foreign-change-detected`、resumable)。以前の実装にあった DEFER 挙動
(wf_66089952 は正しく defer した) が precheck 経路では失われている。

**修正案 A-3 (運用回避 — 即効)**: #220 の運用どおり、コミット前に issue
コメントで files 宣言を追記する。ただし手動運用は恒久策ではない。

### FP-B: `agent({schema})` StructuredOutput 未呼び出し (2 回観測)

**症状**: workflow の subagent が 2 回の in-conversation nudge 後も
StructuredOutput を呼ばず `Error: agent({schema}): subagent completed without
calling StructuredOutput` で workflow 全体が failed (wf_47f070bd,
wf_99ca91ef — 各 ~46-54 万 token waste)。

**修正案 B-1**: nudge を最終 user message として同一 agent を再開 (現行は
fail-fast)。workflow の agent() に `onSchemaMiss: 'retry' | 'fail'` オプション。
**修正案 B-2**: 該当フェーズの schema を緩和 (必須フィールドのみに縮小。
構造化必須フィールドが多いほど LLM が通常テキストで答えてしまう率が上がる
という仮説 — 適用時に検証)。

## 影響集計 (実測)

| run | 結果 | 消費 token |
|---|---|---|
| wf_47f070bd | agent({schema}) failed | 457k |
| wf_99ca91ef | agent({schema}) failed | 541k |
| wf_2000f26f | precheck FAIL (TOCTOU) | 330k |
| wf_15f880f3 | precheck FAIL (TOCTOU) | 351k |
| 計 4 失敗 | — | **~1.68M subagent token** |

## 検証計画 (適用時)

- A-1/A-2: `tests/ticket-prechange-race.test.mjs` — (i) basis 前の uncommitted
  変更は undeclared に数えない、(ii) basis 後の外国変更は defer、
  (iii) 従来どおり真の宣言漏れ (basis 前から存在する未宣言変更) は FAIL。
- B-1: onSchemaMiss: 'fail' 時の現行挙動を維持する回帰テスト +
  retry で復帰するケース。

## 適用手順

CEO 承認後、feature-pipeline.js + ワークフロー agent ランタイム
(`Workflow` script 側の agent() オプション) に分けて適用。
S2-GATE-FALSE-POSITIVE-FIX-PROPOSAL.md (Issue #91/#95/#102) と同じ形式で
Issue 化し、適用済みセクションを追記していく。
