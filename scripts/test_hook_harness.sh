#!/bin/bash
# Assertion-based reliability harness for .claude/hooks/block-dangerous-git.sh
#
# Contract source of truth: .claude/rules/git-guardrails.md (§4 ALWAYS_BLOCK, §6 trunk
# protection, §7 canonical cases). Per §9, this harness and §7 must be updated in
# lockstep — case additions/removals require editing both.
#
# History:
#   2026-06 (original): report-only. Printed observed BLOCK/ALLOW, never compared against
#     an expectation, and always exited 0 — a failing guardrail could not fail the run.
#   2026-09 (Issue #108, T1): assertion-based upgrade.
#     - Every case declares its expected BLOCK/ALLOW from the canonical contract.
#     - The canonical 20 cases (11 BLOCK / 9 ALLOW, §7) are the immutable contract.
#     - §6.2 branch-dependent cases (canonical-A03 "push origin HEAD" / canonical-A04
#       bare push) resolve their expectation AFTER resolving the current branch, exactly
#       like the hook does: BLOCK on master/main, ALLOW on any other branch.
#     - Exit contract (AC-6): 0 iff every case matches, 1 on any mismatch or preflight
#       failure. The harness itself never exits 2 (reserved for the hook's BLOCK).
#     - A hook exit code outside {0, 2} is a CONTRACT-VIOLATION failure even when a
#       naive BLOCK/ALLOW reading would have matched.
#
# TDD RED note (Issue #108 T1, lane: tdd):
#   newcase-a/b/c/d (heredoc body / comment / quoted string literal / DISABLE_GIT_GUARD=1
#   process-env escape hatch) and newcase-j (invalid-JSON fail-closed fallback) are
#   intentionally RED against the current RAW-scan hook: it matches raw substrings
#   anywhere in the command text, never reads DISABLE_GIT_GUARD, and silently ALLOWs
#   (exit 0, empty command) when jq parsing fails. These 5 cases FAIL here until T2
#   (jq-scoped matcher + escape hatch + fail-closed fallback) lands.
#   newcase-e/f/g/h/i pass against the current hook and must stay green after T2.
#
# Portability: HOOK is derived from REPO_ROOT via `git rev-parse --show-toplevel`,
# mirroring scripts/verify_pr_merge_safety.sh L25-37 (set -u, git-presence check,
# REPO_ROOT derivation with fail-closed exit, cd into repo). No machine-specific
# path literals — runs on any checkout / CI / non-Windows machine.

set -u

# --- Preflight: fail-closed on missing prerequisites ---------------------------
if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git not found in PATH" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq not found in PATH" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 1
}
# shellcheck disable=SC2164
cd "$REPO_ROOT"
HOOK="$REPO_ROOT/.claude/hooks/block-dangerous-git.sh"
if [ ! -f "$HOOK" ]; then
  echo "ERROR: hook not found at $HOOK" >&2
  exit 1
fi

TOTAL=0
PASSED=0
FAILED=0
FAILURE_SUMMARY=()

# --- §6.2 branch-dependent expectation resolution ------------------------------
# The hook resolves bare/HEAD push targets against ITS current branch. The harness
# runs in the same repo/worktree, so resolve the same branch and derive the matching
# expectation: master/main => BLOCK, anything else (incl. detached "HEAD") => ALLOW.
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf '')"
case "$CURRENT_BRANCH" in
  master|main) BRANCH_DEPENDENT_EXPECT=BLOCK ;;
  *)           BRANCH_DEPENDENT_EXPECT=ALLOW ;;
esac

# --- Hook invocation helpers ----------------------------------------------------
build_payload() {
  # $1 = command string; emits PreToolUse-style JSON via jq (safe escaping incl. newlines)
  printf '{"tool_input":{"command":%s}}' "$(printf '%s' "$1" | jq -Rs .)"
}

run_hook() {
  # $1 = payload string; pipes it into the hook. Sets HOOK_RC + HOOK_STDERR (stderr only).
  HOOK_STDERR=$(printf '%s\n' "$1" | bash "$HOOK" 2>&1 1>/dev/null)
  HOOK_RC=$?
}

run_env() {
  # run_env <payload> <VAR=VAL>... — invoke the hook with env vars set in the HOOK
  # PROCESS environment (never in the command body). This is the only legitimate
  # DISABLE_GIT_GUARD channel (estimate-approval condition 1: the hook must read the
  # variable from its own process env; a command-body env prefix does not propagate
  # to the hook process and must not disable the guard).
  local payload="$1"; shift
  HOOK_STDERR=$(printf '%s\n' "$payload" | env "$@" bash "$HOOK" 2>&1 1>/dev/null)
  HOOK_RC=$?
}

assert_result() {
  # $1 = expected BLOCK|ALLOW, $2 = case label (carries the command text)
  local expected="$1" label="$2" actual=""
  case "$HOOK_RC" in
    0) actual="ALLOW" ;;
    2) actual="BLOCK" ;;
    *) actual="CONTRACT-VIOLATION(exit=$HOOK_RC)" ;;
  esac
  TOTAL=$((TOTAL + 1))
  if [ "$actual" = "$expected" ]; then
    PASSED=$((PASSED + 1))
    printf 'PASS expect=%-5s got=%-5s %s\n' "$expected" "$actual" "$label"
  else
    FAILED=$((FAILED + 1))
    FAILURE_SUMMARY+=("expect=$expected got=$actual $label | hook_stderr: ${HOOK_STDERR%%$'\n'*}")
    printf 'FAIL expect=%-5s got=%-5s %s\n' "$expected" "$actual" "$label"
  fi
}

expect() {
  # expect <BLOCK|ALLOW> <label> <command>
  run_hook "$(build_payload "$3")"
  assert_result "$1" "$2"
}

expect_env() {
  # expect_env <BLOCK|ALLOW> <label> <VAR=VAL space-separated list> <command>
  local payload
  payload=$(build_payload "$4")
  # shellcheck disable=SC2086  # intentional word split: env spec is a VAR=VAL list
  run_env "$payload" $3
  assert_result "$1" "$2"
}

expect_raw_payload() {
  # expect_raw_payload <BLOCK|ALLOW> <label> <raw stdin text (invalid JSON allowed)>
  # Exercises the hook's fail-closed RAW fallback path (estimate-approval condition 3:
  # when jq structured parsing fails, the hook must fall back to RAW scanning and BLOCK
  # a dangerous payload — never silently ALLOW).
  run_hook "$3"
  assert_result "$1" "$2"
}

echo "hook under test : $HOOK"
echo "current branch  : ${CURRENT_BRANCH:-unknown} (§6.2 branch-dependent expectation: $BRANCH_DEPENDENT_EXPECT)"
echo ""

# --- §7 canonical SHOULD BLOCK (11 cases — contract immutable) ------------------
echo "=== Canonical SHOULD BLOCK (git-guardrails.md §7) ==="
expect BLOCK '[canonical-B01] git push origin master'              'git push origin master'
expect BLOCK '[canonical-B02] git push origin main'                'git push origin main'
expect BLOCK '[canonical-B03] git push origin HEAD:master'         'git push origin HEAD:master'
expect BLOCK '[canonical-B04] git push origin :master'             'git push origin :master'
expect BLOCK '[canonical-B05] git push --force origin feat'        'git push --force origin feat'
expect BLOCK '[canonical-B06] git push -f origin feat'             'git push -f origin feat'
expect BLOCK '[canonical-B07] git push --all'                      'git push --all'
expect BLOCK '[canonical-B08] git push --mirror'                   'git push --mirror'
expect BLOCK '[canonical-B09] git clean -fd'                       'git clean -fd'
expect BLOCK '[canonical-B10] git branch -D feat'                  'git branch -D feat'
expect BLOCK '[canonical-B11] mix lease+force (§5 / AC6)'          'git push --force-with-lease --force origin feat'

# --- §7 canonical SHOULD ALLOW (7 branch-independent cases) ---------------------
echo ""
echo "=== Canonical SHOULD ALLOW — branch-independent (git-guardrails.md §7) ==="
expect ALLOW '[canonical-A01] git push origin feat/x'                        'git push origin feat/x'
expect ALLOW '[canonical-A02] git push -u origin feat/x'                     'git push -u origin feat/x'
expect ALLOW '[canonical-A05] git push origin feat/master-fix'               'git push origin feat/master-fix'
expect ALLOW '[canonical-A06] git push origin release/masterson'             'git push origin release/masterson'
expect ALLOW '[canonical-A07] git push --force-with-lease origin feat'       'git push --force-with-lease origin feat'
expect ALLOW '[canonical-A08] git push --force-with-lease=<ref> origin feat' 'git push --force-with-lease=mainfeat origin feat'
expect ALLOW '[canonical-A09] lease=<expect>:<update> origin feat'           'git push --force-with-lease=abc123:def456 origin feat'

# --- §7 canonical SHOULD ALLOW (2 branch-dependent cases, §6.2) ----------------
echo ""
echo "=== Canonical SHOULD ALLOW — §6.2 branch-dependent (expect=$BRANCH_DEPENDENT_EXPECT on branch '$CURRENT_BRANCH') ==="
expect "$BRANCH_DEPENDENT_EXPECT" '[canonical-A03] git push origin HEAD (branch-dependent)' 'git push origin HEAD'
expect "$BRANCH_DEPENDENT_EXPECT" '[canonical-A04] git push (bare, branch-dependent)'       'git push'

# --- Issue #108 new cases --------------------------------------------------------
# RED targets (expected to FAIL on the current RAW-scan hook, pass after T2): a, b, c, d, j
# Guardrail-pinning cases (must pass now and after T2): e, f, g, h, i
echo ""
echo "=== Issue #108 new cases (T1) ==="

# (a) UC-1: the heredoc BODY contains a dangerous token, but the executable command
#     line does not — heredoc bodies are data, not commands.
HEREDOC_CMD="$(printf '%s\n' "cat <<'EOF'" "docs example: git push --force origin feat (reference text, not executed)" "EOF")"
expect ALLOW '[newcase-a] heredoc body contains dangerous token (UC-1)' "$HEREDOC_CMD"

# (b) a comment-introduced dangerous token is not part of the executable command line.
expect ALLOW '[newcase-b] comment contains dangerous token' 'echo deployed # never run git reset --hard here'

# (c) a quoted string literal argument (echo "push --force") is data, not a command.
expect ALLOW '[newcase-c] quoted string literal contains dangerous token' 'echo "push --force"'

# (d) UC-3: DISABLE_GIT_GUARD=1 set in the HOOK PROCESS env disables the guard
#     (reviewer/verifier lane escape hatch).
expect_env ALLOW '[newcase-d] DISABLE_GIT_GUARD=1 (process env) escape hatch (UC-3)' 'DISABLE_GIT_GUARD=1' 'git push origin master'

# (e) UC-3 counterpart: env unset => guard stays fully active.
expect BLOCK '[newcase-e] DISABLE_GIT_GUARD unset => guard active' 'git push origin master'

# (f) UC-4: lease + unconditional force mixed — lease is stripped, the remaining
#     --force keeps BLOCKing (canonical-B11 contract maintained).
expect BLOCK '[newcase-f] lease+force mix stays BLOCK (UC-4)' 'git push --force-with-lease --force origin feat'

# (g) command substitution contents ARE executed by the shell => stays BLOCK.
expect BLOCK '[newcase-g] command substitution containing dangerous push stays BLOCK' 'echo "danger: $(git push --force origin feat)"'

# (h) segments after && in a compound command ARE executed => stays BLOCK.
expect BLOCK '[newcase-h] echo && dangerous segment stays BLOCK' 'echo start && git reset --hard HEAD~1'

# (i) estimate-approval condition 1 (adversarial): DISABLE_GIT_GUARD=1 as a COMMAND-BODY
#     env prefix must NOT disable the guard — an env prefix does not propagate to the
#     hook process, so this remains a trunk push and must BLOCK.
expect BLOCK '[newcase-i] command-body env prefix is not an escape hatch' 'DISABLE_GIT_GUARD=1 git push origin master'

# (j) estimate-approval condition 3 (fail-closed fallback): invalid JSON whose raw text
#     contains a dangerous payload must fall back to RAW scanning and BLOCK, never
#     silently ALLOW on jq parse failure.
expect_raw_payload BLOCK '[newcase-j] invalid JSON w/ dangerous payload => fail-closed BLOCK' 'not-json-prefix {"tool_input":{"command":"git push --force origin feat"}}'

# (k) estimate-approval condition 2 (gap closure, T2 decision = token-aware): the old
#     RAW substring scan required `push --force` adjacency, so `git push origin --force`
#     (force flag AFTER the remote) was ALLOWED despite being a real force push. T2
#     closes this via token-level --force/-f/--all/--mirror detection inside push
#     segments (decision to be recorded in §7 by T3). RED against the pre-T2 hook.
expect BLOCK '[newcase-k] force flag after remote (token-aware gap closure)' 'git push origin --force feat'

# (l) T2 spec: CRLF (\r) is stripped before parsing (Windows checkout tolerance) — a
#     \r\n between two command lines must not hide the dangerous second segment.
expect BLOCK '[newcase-l] CRLF between segments still detected' "$(printf 'echo ok\r\ngit clean -fd')"

# (m) T2 design decision: trunk protection runs on the push segment FULL token list
#     (fully-quoted tokens included), so quoting the refspec is not a bypass; quoting
#     in a NON-push segment (newcase-c) stays ALLOW.
expect BLOCK '[newcase-m] quoted trunk refspec is not a bypass' 'git push origin "master"'

# --- Summary ----------------------------------------------------------------------
echo ""
echo "=== SUMMARY ==="
echo "total=$TOTAL pass=$PASSED fail=$FAILED"
if [ "$FAILED" -gt 0 ]; then
  echo "FAILURES:"
  for entry in "${FAILURE_SUMMARY[@]}"; do
    echo "  $entry"
  done
  exit 1
fi
echo "ALL CASES PASS"
exit 0
