#!/usr/bin/env bash
set -u
cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)" || exit 1
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
RETRY_MAX="${RETRY_MAX:-20}"
BASELINE="${1:-target/quality/mutants-baseline.json}"

if [ -n "$(git status --porcelain crates 2>/dev/null)" ]; then
  echo "ERROR: crates/ has uncommitted changes — commit or stash before running." >&2
  git status --porcelain crates | head -5 >&2
  exit 1
fi

cleanup_tree() {
  if ! git checkout -- crates 2>&1; then
    echo "ERROR: git checkout -- crates failed — mutated sources may remain. Aborting." >&2
    exit 1
  fi
  if ! git diff --quiet -- crates 2>/dev/null; then
    echo "ERROR: crates/ still dirty after checkout — mutated sources remain. Aborting." >&2
    git status --porcelain crates | head -5 >&2
    exit 1
  fi
  rm -f crates/anaden-engine/Title_*.png 2>/dev/null || true
}

attempt=0
while true; do
  attempt=$((attempt + 1))
  echo "=== mutants attempt #$attempt start $(date -u +%FT%TZ) ==="
  cargo mutants --workspace --in-place --jobserver-tasks 4 --iterate
  rc=$?
  echo "=== cargo mutants rc=$rc $(date -u +%FT%TZ) ==="
  cleanup_tree
  if [ "$rc" -le 1 ]; then
    echo "=== run complete (rc=$rc) — evaluating gates ==="
    break
  fi
  if [ "$attempt" -ge "$RETRY_MAX" ]; then
    echo "=== RETRY_MAX ($RETRY_MAX) reached — giving up with rc=$rc ==="
    exit "$rc"
  fi
  echo "=== abnormal exit (rc=$rc) — retrying in 30s (sources restored) ==="
  sleep 30
done

final_rc=0

if [ -f "$BASELINE" ]; then
  mkdir -p target/quality
  if ! cp "$BASELINE" target/quality/mutants-baseline.json; then
    echo "ERROR: failed to copy baseline from $BASELINE" >&2
    exit 1
  fi
  node scripts/quality/mutants_gate.mjs check
  gate_rc=$?
  echo "=== mutants_gate check rc=$gate_rc ==="
  [ "$gate_rc" -ne 0 ] && final_rc=$gate_rc
  node scripts/quality/mutants_reconcile.mjs target/quality/mutants-baseline.json target/mutants.out/outcomes.json
  recon_rc=$?
  echo "=== reconcile rc=$recon_rc ==="
  [ "$recon_rc" -ne 0 ] && [ "$final_rc" -eq 0 ] && final_rc=$recon_rc
else
  echo "=== baseline not found at $BASELINE — snapshotting instead ==="
  node scripts/quality/mutants_gate.mjs snapshot
  snap_rc=$?
  echo "=== snapshot rc=$snap_rc ==="
  [ "$snap_rc" -ne 0 ] && final_rc=$snap_rc
fi
echo "=== all done (final rc=$final_rc) $(date -u +%FT%TZ) ==="
exit "$final_rc"
