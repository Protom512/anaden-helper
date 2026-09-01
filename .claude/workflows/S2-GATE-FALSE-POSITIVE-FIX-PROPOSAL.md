# S2 Gate 環境依存 False-Positive — 修正提案 (文書化のみ / 適用は CEO 承認経由)

対象: `.claude/workflows/feature-pipeline.js` (S2 evidence gate, R8 diff-inject)
出所: Issue #87 Task 2 — cycle-26〜30 retrospective (org-feedback.md 2026-08-24〜08-26)
作成: 2026-08-26

## 背景・症状

cycle-23〜30 で S2 evidence gate / gate reviewer に環境依存の false-positive・
false-negative が繰り返した。上位 3 構造課題と修正案:

## FP-1: 空または矛盾する DIFF の注入 (最重要) — **applied (P-007 / Issue #91)**

**症状** (org-feedback L247, L255, L281, L286, L288):
- commit 済み slice → working-tree DIFF 空のまま gate 到達 → reviewer が空 diff を評価
- untracked のみの slice → `=== DIFF ===` が空。lane ごとに見える入力が矛盾
  (reliability/extensibility/security は空、performance/governance は untracked を視認)
- 結果: 「実装があるのに NO-GO」(false-positive) と「空なのに vacuous GO」(false-negative) が混在

> **適用済み (2026-08-27)**: 本 FP-1 修正案は Issue #91 (P-007) として実装・適用済み。
> feature-pipeline.js の diff fetch は commit-range diff (HEAD~1..HEAD / merge-base) と
> `git write-tree` の tree hash を併取し、working-tree 空 diff 時は commit-range へ
> fallback、両方空なら fail-closed (gate 短絡)。テスト:
> `tests/gate-commit-range-diff.test.mjs` / `tests/review-gate-diff-range.test.mjs`。
>
> **P-008 (Issue #95) 適用済み (2026-08-27)**: diff-kind lane short-circuit —
> `classifyDiffKind` (docs-only / code / mixed, fail-closed は code 側) が
> docs-only diff の場合 reliability/performance/extensibility lane を short-circuit
> (governance + integration 常時維持)。判定根拠は `.omc/logs/{run-id}/diff-kind-short-circuit.json`
> に永続化。テスト: `tests/gate-diff-kind.test.mjs` / `tests/gate-lane-ownership.test.mjs` /
> `tests/gate-diff-kind-wiring.test.mjs`。
>
> **Issue #102 適用済み (2026-09-01)**: 本書 FP-1/FP-2 の残余修正案
> (diff 収集の単一情報源化) は Issue #102「Gate diff 収集の単一情報源化:
> 決定論的 commit-range fallback + 全 lane 共有スナップショット注入」として
> 実装・適用済み。`buildUnifiedGateDiff` (review-gate-diff.js, 純関数) が
> 決定論的 fallback チェーン (working-tree → HEAD~1..HEAD commit-range →
> merge-base (origin/master...HEAD) → untracked intent-to-add / `git add -N`)
> を単一情報源として解決し、basis・treeHash・snapshot を
> `.omc/logs/{run-id}/gate-diff.json` に永続化、同一 snapshot を全 lane へ注入。
> 空 diff・429 placeholder (明示的文字列完全一致検出) は fail-closed で
> fan-out を拒否。テスト: `tests/review-gate-diff.test.mjs` /
> `tests/gate-diff-unified-docs.test.mjs`。
> **承認経緯**: estimate approval (CTO APPROVE, 2026-09-01 run) を経て適用。
> 本書「適用は CEO 承認経由」注記との権者相違は `.claude/org-feedback.md` に
> 記載済み (承認権者の一元化は retro 課題)。

**修正案** (feature-pipeline.js Commit Gate / S2 セクション):
1. diff 収集ステップで `git status --porcelain` + `git diff HEAD~1..HEAD` を併取し、
   working-tree diff が空で untracked も空なら **commit-range diff に自動フォールバック**。
2. 両方空なら gate を **fail-closed** (evidence 未収集として扱い、green にしない)。
3. untracked のみの場合は `git add -N` を実行してから diff を取り直す
   (intent-to-add diff を注入)。
4. 注入 evidence に `git write-tree` の tree hash を添付し、reviewer 側で
   「diff 空 vs tree 変更あり」の矛盾を検出可能にする。

## FP-2: gate verdict の検証不能性 (偽 evidence 蓋然性)

**症状** (org-feedback L259): verdict が PR 本文の自己申告のみで、PR review や
永続ログが存在しないため verdict-vs-outcome が監査不能。

**修正案**:
1. Merge agent が gate verdict を PR review として投稿 (`gh pr review --request` 相当の
   agent lane 追加、または verdict JSON を `.omc/logs/{run-id}/gate-verdicts.json` に保存)。
2. evidence collector 失敗 (429 等) 時は「未収集」ステータスを明示的にマージ結果に記録。

## FP-3: truncate された大差分 diff

**症状** (org-feedback L294): runner.rs +372 行が切り捨てられ、security reviewer が
PATH 解析検証を Release Review に先送りした。

**修正案**: diff 注入時に per-file 分割。閾値 (例: 300 行) 超のファイルは
「ファイル名 + 変更統計 + 個別 Read 指示」形式で注入する。

## 適用方針

- 本書は**文書化のみ**。feature-pipeline.js への直接適用は別 issue 化し、
  CEO 承認 (estimate 承認条件 2) を経ること。
- FP-1 は P-004 (空コミット precheck) と補完関係。P-004 は「成果物ゼロ」防止、
  FP-1 は「gate への入力欠落」防止。

## 関連

- rule 化済み: `.claude/rules/pipeline-evidence-verification.md`
- 保留中 Self-Improve 案 (S3/S4/S5・R6(b)(c)) は `.claude/pipeline-ledger.json` ceo_queue 参照
