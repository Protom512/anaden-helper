#!/bin/bash
# 危険な git 操作を PreToolUse でブロック（git-guardrails-claude-code スキール）。
# 例外: Wiki サブリポジトリ(docs/anaden-helper.wiki)内の「通常の git push」のみ
#        プロジェクトルールで承認済みのため許可。--force 等は Wiki でも拒否。

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command')

# Wiki サブリポジトリを対象とする操作か（パス名で判定）
if echo "$COMMAND" | grep -qE "anaden-helper\.wiki"; then
  IS_WIKI=yes
else
  IS_WIKI=no
fi

# 常にブロック（Wiki でも拒否）
DANGEROUS_PATTERNS=(
  "git reset --hard"
  "git clean -fd"
  "git clean -f"
  "git branch -D"
  "git checkout \."
  "git restore \."
  "push --force"
  "push -f"
  "reset --hard"
)

# git push は Wiki 以外でブロック（Wiki の通常 push は許可、--force は上記で常に拒否）
if [ "$IS_WIKI" != "yes" ]; then
  DANGEROUS_PATTERNS+=("git push")
fi

for pattern in "${DANGEROUS_PATTERNS[@]}"; do
  if echo "$COMMAND" | grep -qE "$pattern"; then
    echo "BLOCKED: '$COMMAND' matches dangerous pattern '$pattern'. The user has prevented you from doing this." >&2
    exit 2
  fi
done

exit 0
