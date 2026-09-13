#!/bin/bash
# git-guardrails — 危険な git 操作を PreToolUse でブロックする最小フック。
#
# 2026-09-13 最小化改訂 (Issue #197 の発散を受けてのユーザー判断):
#   - 旧実装 (jq シェル構造解析・cd/-C repo 追跡・trunk push 判定・49 ケース
#     harness) を全面廃止。bash でシェル構文を完全に解釈することは原理的に
#     不可能で、「直すほど境界ケースが増殖し仕事が自己増幅する」ため。
#   - trunk 保護 (master への直接 push・強制 push・削除) は GitHub ruleset
#     "master-trunk-protection" (id 23145067: deletion / non_fast_forward /
#     pull_request 必須) がサーバー側で担う。ローカルでは再実装しない。
#   - 本フックの責務は「GitHub では防げないローカル破壊」のみ — ALWAYS_BLOCK
#     パターンの RAW 文字列照合。
#
# トレードオフ (明示受諾):
#   - 誤 BLOCK あり: heredoc・コメント・コミットメッセージ内のパターン言及も
#     ブロックする。その場合はコマンドを分割するか DISABLE_GIT_GUARD ハッチを使う。
#   - 誤 ALLOW あり: クォート・変数展開等で難読化された危険操作は検出しない。
#     脅威モデルは「敵対者」ではなく「AI エージェントの誤操作」であり、誤操作は
#     平文で書かれるため実用上十分。
#
# 正準ケース一覧 (BLOCK/ALLOW の真実の源): scripts/test_hook_harness.sh
# 契約文書: .claude/rules/git-guardrails.md

# ── エスケープハッチ ── フックプロセスの環境変数のみ有効。
#    コマンド本文中の `DISABLE_GIT_GUARD=1 git ...` 前置きは伝播しないため無効。
#    使用は無言にしない (stderr へ監査行)。
if [ "${DISABLE_GIT_GUARD:-}" = "1" ]; then
  HATCH_INPUT=$(cat)
  echo "GIT_GUARD_DISABLED: escape hatch used (DISABLE_GIT_GUARD=1) — command NOT scanned: ${HATCH_INPUT:0:200}" >&2
  exit 0
fi

INPUT=$(cat)
COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
# fail-closed: jq 解析失敗 (不正 JSON 等) では RAW 入力全文をスキャン対象にする。
[ -n "$COMMAND" ] || COMMAND="$INPUT"
# CRLF は照合前に strip (Windows 環境対策)。
COMMAND=${COMMAND//$'\r'/}
# --force-with-lease は安全な force push なので照合前に除去 (bare/=ref/=expect:update の 3 形式)。
COMMAND=$(printf '%s' "$COMMAND" | sed -E 's/--force-with-lease(=[^[:space:]]*)?//g')

# ── ALWAYS_BLOCK — 全 repo (ネスト wiki repo 含む) で常にブロック ──
ALWAYS_BLOCK_PATTERNS=(
  "reset --hard"      # git reset --hard / 裸 reset --hard (作業ツリー破壊)
  "git clean -f"      # git clean -f / -fd (未追跡ファイル削除)
  "git branch -D"     # ブランチ強制削除
  "git checkout ."    # 作業ツリー変更の一括破棄
  "git restore ."     # 同上 (restore 形式)
  "push --force"      # 無条件 force push (--force-with-lease は除去済み)
  "push -f"           # 同上 (短縮形)
  "push --all"        # 全ブランチ一括 push
  "push --mirror"     # ミラー push
)

for p in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
  case "$COMMAND" in
    *"$p"*)
      echo "BLOCKED: '$COMMAND' — 危険な git 操作パターン '$p' を検出 (ローカル破壊/強制 push 防止)。誤検知の場合はコマンドを分割するか DISABLE_GIT_GUARD=1 (フックプロセス環境変数) を使用してください。" >&2
      exit 2
      ;;
  esac
done

exit 0
