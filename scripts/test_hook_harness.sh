#!/bin/bash
# Assertion-based harness for .claude/hooks/block-dangerous-git.sh (2026-09-13 最小化版)
#
# 契約: .claude/rules/git-guardrails.md — ALWAYS_BLOCK RAW パターン照合のみ。
# trunk 保護 (master 直接 push・強制 push・削除のサーバー側阻止) は GitHub ruleset
# "master-trunk-protection" (id 23145067) が担当。ローカルフックは push の
# refspec / ブランチ / 対象 repo 判定を一切行わない。
#
# 規約 (git-guardrails.md 正準ケース表 と lockstep):
#   - 各ケースは期待値 (BLOCK/ALLOW) を assertion する。全一致で exit 0、
#     不一致・precheck 失敗で exit 1。harness 自身は exit 2 を使わない。
#   - フックの終了コードが {0,2} 以外は期待値不一致として FAIL 扱い
#     (CONTRACT-VIOLATION 検出)。

HOOK="$(cd "$(dirname "$0")/.." && pwd)/.claude/hooks/block-dangerous-git.sh"
PASS=0 FAIL=0

assert_result() {  # $1: Case ID, $2: 期待値 (BLOCK|ALLOW), $3: コマンド
  local id="$1" expect="$2" cmd="$3" rc
  printf '%s' "$cmd" | jq -Rs '{tool_input: {command: .}}' | bash "$HOOK" >/dev/null 2>&1
  rc=$?
  if { [ "$expect" = "BLOCK" ] && [ "$rc" = "2" ]; } || { [ "$expect" = "ALLOW" ] && [ "$rc" = "0" ]; }; then
    PASS=$((PASS+1)); echo "PASS expect=$expect [$id]"
  else
    FAIL=$((FAIL+1)); echo "FAIL expect=$expect got_rc=$rc [$id] cmd=$cmd"
  fi
}

# ── precheck ──
bash -n "$HOOK" || { echo "PRECHECK-FAIL: hook syntax"; exit 1; }
command -v jq >/dev/null || { echo "PRECHECK-FAIL: jq missing"; exit 1; }

# ── ALWAYS_BLOCK パターン (11) ──
assert_result case-01 BLOCK 'git reset --hard HEAD~1'
assert_result case-02 BLOCK 'reset --hard'
assert_result case-03 BLOCK 'git clean -fd'
assert_result case-04 BLOCK 'git clean -f'
assert_result case-05 BLOCK 'git branch -D feat/x'
assert_result case-06 BLOCK 'git checkout .'
assert_result case-07 BLOCK 'git restore .'
assert_result case-08 BLOCK 'git push --force origin feat/x'
assert_result case-09 BLOCK 'git push -f origin feat/x'
assert_result case-10 BLOCK 'git push --all'
assert_result case-11 BLOCK 'git push --mirror'

# ── ALLOW (6) ──
assert_result case-12 ALLOW 'git push origin feat/issue-1'
assert_result case-13 ALLOW 'cd docs/anaden-helper.wiki && git push origin master'   # GitHub Wiki 出版経路 (ネスト repo)
assert_result case-14 ALLOW 'git push --force-with-lease origin feat/x'
assert_result case-15 ALLOW 'git push --force-with-lease=abc123:def456 origin feat/x'
assert_result case-16 ALLOW 'cargo nextest run --workspace'
assert_result case-17 ALLOW 'echo "run tests please"'

# ── 明示受諾の誤 BLOCK pin (RAW 照合の既知トレードオフ) ──
# 文言内の "reset --hard" 言及もブロックする。誤検知時はコマンド分割 or ハッチ利用。
assert_result case-18 BLOCK 'git commit -m "note: do not reset --hard on release branch"'

# ── fail-closed: 不正 JSON stdin でも危険 payload は RAW 全文スキャンで BLOCK ──
printf '%s' 'not-json-prefix {"tool_input":{"command":"git reset --hard HEAD~1"}}' | bash "$HOOK" >/dev/null 2>&1
rc=$?
if [ "$rc" = "2" ]; then PASS=$((PASS+1)); echo "PASS expect=BLOCK [case-19]"; else FAIL=$((FAIL+1)); echo "FAIL expect=BLOCK got_rc=$rc [case-19]"; fi

# ── エスケープハッチ: フックプロセス環境変数 DISABLE_GIT_GUARD=1 → exit 0 + stderr 監査行 ──
printf '%s' '{"tool_input":{"command":"git reset --hard"}}' | env DISABLE_GIT_GUARD=1 bash "$HOOK" >/dev/null 2>/tmp/hatch-stderr.txt
hrc=$?
if [ "$hrc" = "0" ] && grep -q "GIT_GUARD_DISABLED" /tmp/hatch-stderr.txt; then PASS=$((PASS+1)); echo "PASS hatch [case-20]"; else FAIL=$((FAIL+1)); echo "FAIL hatch rc=$hrc [case-20]"; fi
rm -f /tmp/hatch-stderr.txt

echo "=== SUMMARY ==="
echo "total=$((PASS+FAIL)) pass=$PASS fail=$FAIL"
[ "$FAIL" = 0 ] || exit 1
echo "ALL CASES PASS"
exit 0
