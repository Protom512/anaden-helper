#!/bin/bash
# 危険な git 操作を PreToolUse でブロック（git-guardrails-claude-code スキール）。
#
# 2026-06-24 改訂: feature ブランチへの git push を許可し、master/main（本線）への
#   直接 push のみブロックする（org-feedback #150/#151: push のたびに CEO override が
#   必要となる single-point-of-failure を解消）。破壊的リセット/削除・force push・
#   --all/--mirror は Wiki 無関係に常に拒否（本線保護維持）。

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command')

# ── 常にブロック（Wiki でも拒否）─────────────────────────────────────────
# 破壊的リセット/削除 + force push + 全ブランチ一括 push
#   ※ --all / --mirror は feature だけでなく master/main も含めて push するため本線保護上ブロック
ALWAYS_BLOCK_PATTERNS=(
  "git reset --hard"
  "git clean -fd"
  "git clean -f"
  "git branch -D"
  "git checkout \."
  "git restore \."
  "push --force"
  "push -f"
  "reset --hard"
  "push --all"
  "push --mirror"
)

# ALWAYS_BLOCK 判定は --force-with-lease を benign flag として事前 strip した COPY で行う。
# 元の COMMAND は L38+ の refspec / bare-push 判定で再利用するため破壊しない(Option B)。
#   ※ push --force / push -f (無条件 force push) はこの COPY 上でも一致して BLOCK される。
#   ※ --force-with-lease は strip 済みなので誘爆せず、feature ブランチ上の push は後段で ALLOW される。
COMMAND_FOR_ALWAYS_BLOCK=$(echo "$COMMAND" | sed -E 's/[[:space:]]+--force-with-lease([[:space:]]|$)/ /g')

for pattern in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
  if echo "$COMMAND_FOR_ALWAYS_BLOCK" | grep -qE "$pattern"; then
    echo "BLOCKED: '$COMMAND' matches dangerous pattern '$pattern'. The user has prevented you from doing this." >&2
    exit 2
  fi
done

# ── git push の本線(master/main)保護 ─────────────────────────────────────
# feature ブランチへの push は許可。本線(master/main)への直接 push のみブロック。
if echo "$COMMAND" | grep -qE "(^|[[:space:]])git push"; then
  BLOCK=no
  REASON=""

  # (1) refspec に master/main がスタンドアロントークンとして含まれる → 本線 push をブロック。
  #     例: `git push origin master`, `git push origin HEAD:master`, `git push origin :master`
  #     境界文字類 [^[:alnum:]./_-] で挟むことで、feat/master-fix 等 branch 名内の master は誘爆しない。
  if echo "$COMMAND" | grep -qE "(^|[^[:alnum:]./_-])(master|main)([^[:alnum:]./_-]|$)"; then
    BLOCK=yes
    REASON="push の refspec が master/main を指している"
  fi

  # (2) 裸 push（`git push` / `git push origin` / `git push origin HEAD`）は現在ブランチを
  #     リモートへ push する → 現在ブランチが master/main ならブロック。
  if [ "$BLOCK" = "no" ]; then
    # refspec 抽出: `git push`・global flags・remote・スタンドアロン HEAD を除去し、残ったトークンを refspec とみなす。
    #   残りが空 = refspec 無し（現在ブランチを push）。残りがあれば明示的な refspec（(1)で本線判定済み、feature は許可）。
    refspec=$(echo "$COMMAND" | sed -E \
      -e 's/^git push//' \
      -e 's/[[:space:]]+(-u|--set-upstream|--force-with-lease|--tags|--no-tags|--dry-run|-n|--quiet|-q|--verbose|-v|--follow-tags)//g' \
      -e 's/[[:space:]]+origin([[:space:]]|$)/ /g' \
      -e 's/[[:space:]]+https?:\/\/[^[:space:]]+//g' \
      -e 's/[[:space:]]+git@[^[:space:]]+//g' \
      -e 's/(^|[[:space:]])HEAD([[:space:]]|$)/ /g' \
      -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//')
    if [ -z "$refspec" ]; then
      CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
      if [ "$CURRENT_BRANCH" = "master" ] || [ "$CURRENT_BRANCH" = "main" ]; then
        BLOCK=yes
        REASON="現在ブランチ '$CURRENT_BRANCH' 上の裸 push（本線へ直接 push される）"
      fi
    fi
  fi

  if [ "$BLOCK" = "yes" ]; then
    echo "BLOCKED: '$COMMAND' — $REASON。feature ブランチへの push は許可されますが、master/main への直接 push は禁止です。PR 経由でマージしてください。" >&2
    exit 2
  fi
fi

exit 0
