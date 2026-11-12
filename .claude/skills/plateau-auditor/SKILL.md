---
name: plateau-auditor
description: 監査役サブエージェント (plateau auditor) パターン。run / pipeline / 実装ループが停滞 (plateau) したときに、記録を「過去に効いた手」の観点台帳と突き合わせて未着手観点を抽出する。停滞の判定 (ばらつき帯付き)・観点台帳の解析・監査プロンプト生成・レポート回収は scripts/plateau-auditor.js を使う。トリガー例: "停滞してる""頭打ち""plateau""監査役""未着手の観点""観点台帳"
---

# Plateau Auditor — 停滞時の監査役サブエージェント

出典: ISUCON2026 優勝チームの準備編
(<https://zenn.dev/luvtechno/articles/isucon2026-preparation>, 2026-11-11 公開)。
3ヶ月・159 本の評価 run から得られた「監査役サブエージェント」パターンを、
本リポジトリの evidence 規約 (`.claude/rules/pipeline-evidence-verification.md`)
に合わせて移植したもの。

## パターン概要

run が停滞 (plateau) したとき、監査役サブエージェントを起動し、
run 記録を「観点台帳」と突き合わせて **未着手の観点** を返させる。

- 観点台帳は「過去の最高 run でスコアを伸ばしたのに、別の run では出なかった手」を
  **対象名を伏せた 1 行** ずつで蓄積する。
- 伏せ字 (redaction) の理由: 対象名 (ファイルパス・PR 番号・ブランチ名) を含めると
  監査役がその具体物にアンカーして、観点としての一般性を失う。
- 出力は最大 5 件。書かせる量を増やすと記録が肥大化する
  (出典記事では 47KB へ膨らんだ教訓) ため、上位観点に絞る。

## モジュール

実装: `scripts/plateau-auditor.js` / テスト: `tests/plateau-auditor.test.mjs`
(ともに本 SKILL.md と同じディレクトリ配下。feature-pipeline 本体からは独立 — opt-in)。

```bash
node --test .claude/skills/plateau-auditor/tests/plateau-auditor.test.mjs
```

| 関数 | 役割 | fail-closed 挙動 |
| --- | --- | --- |
| `parseViewpointLedger(text)` | 台帳 markdown を解析。カテゴリ見出し (`##`) + 1観点1行 (`- `) | 空行・長すぎ (>200字)・伏せ字違反は例外ではなく `errors` に記録 |
| `redactionViolations(text)` | 伏せ字違反の検出 (PR番号 `#\d{2,}`・コードパス・ブランチ名) | — |
| `filterUntried(viewpoints, tried)` | 正規化 (小文字化・空白圧縮) した包含一致で未着手を抽出 | — |
| `isBeyondVariance(newBest, priorBest, variancePct)` | ばらつき帯を超えた更新か | 不正入力は false |
| `detectPlateau(scores, variancePct, window)` | 直近 `window` 回 (既定4) がばらつき帯以内なら停滞 | 不正入力は false |
| `buildAuditorPrompt(input)` | 監査プロンプト生成 (`UNTRIED:` / `NONE` 出力契約を添付) | 空台帳・全件試行済みは `ok:false` |
| `parseAuditorReport(text)` | 監査役の出力を回収 | `UNTRIED:`/`NONE` が無い・6件超は `ok:false` |

既定のばらつき帯 21% は出典記事の実測 (同一 commit の 58 回ベンチでスコア幅 21%) に由来する。
本プロジェクトで使う際は、同一構成の再測定実績があればその値に差し替えること
(「改善」と「ばらつき」の区別は実測ベースで行う — 出典記事 9/8〜9/9 の教訓)。

## 使い方 (opt-in)

1. `detectPlateau(最近の実測値列, variancePct, window)` で停滞を判定する。
   対象の実測値は run の性質に合わせる (gate 再試行回数、リワーク率、ablation の効果量など)。
2. 停滞したら観点台帳を `parseViewpointLedger` で解析する。台帳は run をまたいで
   machine-local に保持し、run の振り返り (retrospective-codify / org-retrospective) の
   たびに「効いたのに出なかった手」を 1 行ずつ足す。
3. `buildAuditorPrompt` で subagent を起動し、`parseAuditorReport` で回収する。
   `ok:false` の場合は **監査は行われなかった** として扱い、
   「該当なし = 台帳が空だった」と green 払いしないこと
   (evidence 規約 §3 の未収集明示と同型)。
4. 回収した未着手観点は「未着手」として記録し、未着手が残るうちに先に試させる。

## 出典記事の教訓と本リポジトリの適用状況

| 記事の教訓 | 本リポジトリでの状態 |
| --- | --- |
| 監査役サブエージェント (plateau 時の観点突き合わせ) | **本 skill で導入** (`scripts/plateau-auditor.js`) |
| verdict は実 verdict の interpolation で PR 本文へ (固定文字列 "all GO" 禁止) | 適用済み — Issue #152 |
| 空 diff / 未収集 evidence は fail-closed | 適用済み — evidence 規約 §2・§3 |
| 同一構成再測定のばらつきで説明できない更新だけを改善と数える | 適用済み — ablation program の negative control (`isBeyondVariance` はその判定ヘルパー) |
| 指示は「bare で回して失敗した場面に手当てを足す」(bare-first) | 適用済み — skill ablation program (run 57 まで全 lane 消化) |
| 見張りの無音検知がバックグラウンド書き込みに騙される / 入力待ちを完了と誤認 | 適用済み — live-session gate (DEF-008〜013) |
| メトリクスはログに残すだけでなく能動的に渡す (到達率 6.4%→98.3%) | 部分適用 — gate verdict は release prompt へ interpolation されるが、run 中間メトリクスの能動配送は未整備 (future work) |
