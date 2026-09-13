# Git Guardrails — 最小契約 (2026-09-13 改訂)

## 背景 (なぜ最小化したか)

旧契約 (jq シェル構造解析・cd/-C repo 追跡・trunk push 判定・49 ケース harness)
は、bash でシェル構文を完全に解釈することは原理的に不可能であるため、修正のたびに
境界ケースが増殖し保守作業が自己増幅した (Issue #197 の発散)。2026-09-13 の
ユーザー判断により**責務を縮小**した:

| 責務 | 担当 |
|---|---|
| trunk 保護 (master 直接 push・強制 push・削除の阻止) | **GitHub ruleset `master-trunk-protection` (id 23145067・サーバー側)** — deletion / non_fast_forward / pull_request 必須 (承認 0 件・ソロ運用) |
| ローカル破壊の阻止 (`reset --hard` / `clean -f` 等) | **本フック** (GitHub では原理的に防げない) |

## フック仕様 (`.claude/hooks/block-dangerous-git.sh`)

- PreToolUse (Bash ツール) で起動。stdin JSON から `.tool_input.command` を抽出
  (jq 失敗時は RAW 入力全文をスキャン = fail-closed)。`\r` strip 後、
  `--force-with-lease`(3 形式) を除去した文字列に対して RAW 部分一致で照合。
- **ALWAYS_BLOCK パターン (9)**: `reset --hard` / `git clean -f` / `git branch -D` /
  `git checkout .` / `git restore .` / `push --force` / `push -f` / `push --all` /
  `push --mirror`。全 repo (ネスト wiki repo 含む) で常時ブロック。
- 終了コード: `0` = ALLOW / `2` = BLOCK (stderr に理由)。それ以外は契約外。
- エスケープハッチ: フックプロセス環境変数 `DISABLE_GIT_GUARD=1` で全免除
  (exit 0 + stderr 監査行)。コマンド本文中の env 前置きは無効。

## 明示受諾のトレードオフ

- **誤 BLOCK あり**: heredoc・コメント・コミットメッセージ内のパターン言及も
  ブロックする (harness `case-18` が pin)。対処: コマンドを分割するかハッチを使う。
- **誤 ALLOW あり**: クォート・変数展開で難読化された危険操作は検出しない。
  脅威モデルは「敵対者」でなく「AI エージェントの誤操作」(誤操作は平文で書かれる)。
- push の refspec・ブランチ・対象 repo 判定は**一切行わない** (GitHub 側で保護)。
  GitHub Wiki (`docs/anaden-helper.wiki`・master 直接 push が唯一の出版経路) は
  trunk 判定がないため自明に ALLOW (旧 Issue #197 の問題は構造的に消滅)。

## 正準ケース一覧 (真実の源: `scripts/test_hook_harness.sh`)

case-01〜11 = ALWAYS_BLOCK 各パターン / case-12〜17 = ALLOW (feature push・wiki
push・force-with-lease・通常コマンド) / case-18 = 誤 BLOCK 受諾 pin /
case-19 = fail-closed (不正 JSON) / case-20 = ハッチ (exit 0 + stderr 監査行)。
計 20 assertion — BLOCK 12 / ALLOW 6 / 特殊 2。harness は全一致で exit 0。

## 改訂時チェックリスト

- [ ] harness ケースと本書の正準ケース一覧を lockstep 更新 (件数は harness 実行の
      実測値から記載)
- [ ] `bash scripts/test_hook_harness.sh` が exit 0
- [ ] ALWAYS_BLOCK 追加時は「GitHub では防げないローカル破壊」に該当するか再確認
      (push 系の trunk 判定をローカルに戻さない — それは GitHub ruleset の責務)

## 履歴

- 2026-06: 初版 (RAW grep・report-only)
- 2026-09-04/05: Issue #108 系 — jq 構造解析・49 ケース (のちに保守発散)
- 2026-09-13: 最小化 (本契約)。Issue #197 は「問題の構造的消滅」として解決。
