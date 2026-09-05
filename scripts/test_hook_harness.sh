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
#   2026-09-05 (Issue #108 remediation, cycle-44 majors — T1 RED phase):
#     - AC-2 quoted-flag: newcase-q/r/s/t/u/v pin BLOCK for fully-quoted FLAG tokens
#       (`git reset "--hard"` etc.) — RED at head 069d16d because the (A) ALWAYS_BLOCK
#       match runs on the plain (unquoted-token) list only, so a quoted flag token is
#       invisible to it. newcase-o/p pin push-segment quoted --force as BORN-GREEN
#       regression pins (the (B) full-token scan already blocks it — estimate-approval
#       condition 1 correction: these were born-green at head, NOT RED), and newcase-w
#       pins multi-word quoted DATA inside a git segment as ALLOW (false-positive-zero:
#       the quoted-flag detection added by T2/T3 must not regress to raw-substring
#       matching).
#     - AC-3 audit trail: AUDIT_MARKER contract — the DISABLE_GIT_GUARD=1 hatch must
#       leave a stderr audit trail (newcase-d2; RED at head: the hatch exits 0 with
#       EMPTY stderr), and the hostile command-body env prefix must NOT carry the
#       marker (newcase-i2; audit trail only on genuine hatch use).
#     - AC-4 heredoc→interpreter bypass: newcase-n pins BLOCK (primary implementation:
#       interpreter-argument scanning). FLIP RULE: if T3 instead adopts the §8
#       explicit-acceptance-enumeration fallback, flip this expectation to ALLOW in
#       the same lockstep commit (with the §7 pin recorded by T4).
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

# --- AC-3 audit-trail marker contract (Issue #108 remediation, cycle-44 major-3) --
# When the DISABLE_GIT_GUARD=1 escape hatch fires, the hook MUST emit this marker to
# stderr (audit trail) — the hatch must never be silent. T3 implements the emission;
# newcase-d2 asserts presence on genuine hatch use, newcase-i2 asserts absence when
# the guard stays active (hostile command-body env prefix).
AUDIT_MARKER='GIT_GUARD_DISABLED'

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

assert_stderr_marker() {
  # assert_stderr_marker <present|absent> <label> — PASS iff HOOK_STDERR contains
  # ($1=present) / does not contain ($1=absent) $AUDIT_MARKER. Composes with
  # expect / expect_env / expect_raw_payload: call it immediately AFTER the
  # exit-code assertion to check the SAME invocation's stderr (HOOK_STDERR is
  # reused, the hook is NOT re-run).
  local want="$1" label="$2" found="absent"
  case "$HOOK_STDERR" in
    *"$AUDIT_MARKER"*) found="present" ;;
  esac
  TOTAL=$((TOTAL + 1))
  if [ "$found" = "$want" ]; then
    PASSED=$((PASSED + 1))
    printf 'PASS stderr[%s] %s\n' "$want" "$label"
  else
    FAILED=$((FAILED + 1))
    FAILURE_SUMMARY+=("stderr[$want] audit marker '$AUDIT_MARKER' not satisfied $label | hook_stderr: ${HOOK_STDERR%%$'\n'*}")
    printf 'FAIL stderr[%s] %s\n' "$want" "$label"
  fi
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

# (d2) AC-3 (cycle-44 major-3): the hatch must not be SILENT — using it leaves an
#      audit trail on stderr ($AUDIT_MARKER). RED at head 069d16d: the hatch exits 0
#      with EMPTY stderr (verified 0-byte). T3 implements the trail; T4 pins §7.
assert_stderr_marker present '[newcase-d2] escape hatch leaves stderr audit trail (AC-3)'

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

# (i2) AC-3 counterpart: the audit trail must appear ONLY on genuine hatch use —
#      a hostile command-body env prefix leaves the guard active (BLOCK above) and
#      its stderr must NOT carry the audit marker. Born-green at head.
assert_stderr_marker absent '[newcase-i2] hostile env prefix BLOCK carries no audit marker (AC-3)'

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

# --- Issue #108 remediation (cycle-44 majors) — T1 RED phase (AC-2/AC-3/AC-4) ------
# RED targets (expected to FAIL on the current head 069d16d hook; T2/T3 turn them
# green): n (heredoc→interpreter bypass, AC-4), q/r/s/t/u/v (quoted-flag ALWAYS_BLOCK
# surface, AC-2), d2 (DISABLE_GIT_GUARD audit trail, AC-3 — silent 0-byte stderr).
# Guardrail-pinning cases (must pass now and stay green through T2/T3): o, p
# (push-segment quoted force flags — born-green per estimate-approval condition 1),
# w (multi-word quoted data in a git segment stays ALLOW), i2 (no audit marker on
# hostile env prefix).
echo ""
echo "=== Issue #108 remediation new cases (T1 RED phase, cycle-44 majors) ==="

# (n) AC-4 (cycle-44 major-4): a heredoc feeding an INTERPRETER (`bash <<'EOF'`)
#     executes the body. The tokenizer correctly treats heredoc bodies as data
#     (newcase-a: `cat <<'EOF'`), but when the consuming command is an interpreter
#     the body becomes a bypass vector. Primary pin = BLOCK (interpreter-argument
#     scanning, T3). FLIP RULE: if T3 adopts the §8 explicit-acceptance-enumeration
#     fallback instead, flip this expectation to ALLOW in the same lockstep commit.
HEREDOC_BYPASS_CMD="$(printf '%s\n' "bash <<'EOF'" 'git reset --hard HEAD~1' 'EOF')"
expect BLOCK '[newcase-n] heredoc-to-interpreter bypass is blocked (AC-4)' "$HEREDOC_BYPASS_CMD"

# (o/p) AC-2 born-green regression pins (estimate-approval condition 1): the push
#     segment's (B) full-token scan already BLOCKs a quoted --force — quoting a push
#     flag is NOT a bypass at head and must remain blocked through T2/T3.
expect BLOCK '[newcase-o] push quoted --force before remote (born-green pin)' 'git push "--force" origin feat'
expect BLOCK '[newcase-p] push quoted --force after remote (born-green pin)'  'git push origin "--force" feat'

# (q-s) AC-2 RED: NON-push ALWAYS_BLOCK surface. The (A) pattern match runs on the
#     plain (unquoted-token) list only, so a fully-quoted flag token (`--hard`,
#     `-fd`, `-D`) is invisible to it and these are ALLOWED at head. T2/T3 close
#     this by treating a fully-quoted token with an exact flag SHAPE as a flag —
#     while multi-word quoted prose (newcase-w) must stay data/ALLOW.
expect BLOCK '[newcase-q] git reset "--hard" (quoted-flag, dq)'   'git reset "--hard"'
expect BLOCK '[newcase-r] git clean "-fd" (quoted-flag, dq)'      'git clean "-fd"'
expect BLOCK '[newcase-s] git branch "-D" feat (quoted-flag, dq)' 'git branch "-D" feat'

# (t-v) AC-2 RED, single-quote variants of the same three quoted-flag surfaces.
expect BLOCK '[newcase-t] git reset --hard (quoted-flag, sq)'   "git reset '--hard'"
expect BLOCK '[newcase-u] git clean -f (quoted-flag, sq)'       "git clean '-f'"
expect BLOCK '[newcase-v] git branch -D feat (quoted-flag, sq)' "git branch '-D' feat"

# (w) AC-2 false-positive-zero pin: a MULTI-WORD quoted string argument inside a git
#     segment is DATA (newcase-c, but within a git command). The quoted-flag
#     detection added by T2/T3 must distinguish a quoted FLAG token (exact `--hard` /
#     `-fd` / `-D` shape → flag) from quoted PROSE containing danger words (→ data).
#     Born-green at head; guards T2/T3 against regressing to raw-substring matching.
expect ALLOW '[newcase-w] git commit -m quoted prose w/ danger words stays ALLOW' 'git commit -m "never run git reset --hard here"'

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
