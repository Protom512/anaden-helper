#!/bin/bash
# Reliability test harness for block-dangerous-git.sh
# Feeds synthetic tool_input JSON and reports BLOCK/ALLOW per case.
HOOK="C:/Users/black/git-repo/anaden-helper/.claude/hooks/block-dangerous-git.sh"
run() {
  local desc="$1"; local cmd="$2"
  local json
  json=$(printf '{"tool_input":{"command":%s}}' "$(printf '%s' "$cmd" | jq -Rs .)")
  local out rc
  out=$(echo "$json" | bash "$HOOK" 2>&1)
  rc=$?
  if [ "$rc" -eq 0 ]; then echo "ALLOW | $desc"; else echo "BLOCK | $desc"; fi
}

echo "=== SHOULD BLOCK (security guardrail) ==="
run "push origin master"        "git push origin master"
run "push origin main"          "git push origin main"
run "push HEAD:master"          "git push origin HEAD:master"
run "delete master refspec"     "git push origin :master"
run "force push feat"           "git push --force origin feat"
run "-f push feat"              "git push -f origin feat"
run "push --all"                "git push --all"
run "push --mirror"             "git push --mirror"
run "clean -fd"                 "git clean -fd"
run "branch -D"                 "git branch -D feat"

echo ""
echo "=== SHOULD ALLOW (feature push / usability) ==="
run "push origin feat/x"        "git push origin feat/x"
run "push -u origin feat/x"     "git push -u origin feat/x"
run "push origin HEAD"          "git push origin HEAD"
run "bare push"                 "git push"
run "push origin feat/master-fix"   "git push origin feat/master-fix"
run "push release/masterson"    "git push origin release/masterson"
run "push --force-with-lease feat"  "git push --force-with-lease origin feat"
