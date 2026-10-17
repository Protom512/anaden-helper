#!/usr/bin/env bash
# Deterministic bench suite (Issue #214, Phase 0 / T5).
#
# Primary metric: peak RSS per workload (scripts/quality/rss_bench.ps1,
# Windows-deterministic). Secondary: criterion bench (log_snapshot_bench)
# with a named baseline for pre/post-refactor comparison. hyperfine wall
# times are reference-only (--warmup + multiple runs, still noisy on Windows).
#
# Usage:
#   ./scripts/quality/bench.sh                # release build, RSS x5, criterion, hyperfine
#   BENCH_PROFILE=debug ./scripts/quality/bench.sh
#   BENCH_SKIP_CRITERION=1 ./scripts/quality/bench.sh   # RSS + hyperfine only
#
# Outputs:
#   target/quality/bench/routine-bench.toml         deterministic workload fixture
#   target/quality/bench/rss-<name>.json            peak RSS reports
#   target/quality/bench/hyperfine-routine.json     wall-time reference (optional)
#   target/criterion/                               criterion data (--save-baseline)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

PROFILE="${BENCH_PROFILE:-release}"
REPEAT="${BENCH_REPEAT:-5}"
CRITERION_BASELINE="${BENCH_CRITERION_BASELINE:-complexity-phase0}"
OUT="target/quality/bench"
mkdir -p "$OUT"

# --- 1) build the CLI (cap build jobs: cold wgpu/eframe parallel builds OOM) ---
JOBS="${CARGO_BUILD_JOBS:-4}"
if [[ "$PROFILE" == "release" ]]; then
  echo "bench: building anaden-cli (release, jobs=$JOBS)"
  CARGO_BUILD_JOBS="$JOBS" cargo build -p anaden-cli --release
  CLI="target/release/anaden.exe"
else
  echo "bench: building anaden-cli (debug, jobs=$JOBS)"
  CARGO_BUILD_JOBS="$JOBS" cargo build -p anaden-cli
  CLI="target/debug/anaden.exe"
fi
[[ -f "$CLI" ]] || { echo "bench: CLI binary not found: $CLI" >&2; exit 2; }
CLI_W="$(cygpath -w "$ROOT/$CLI")"
ROOT_W="$(cygpath -w "$ROOT")"
echo "bench: CLI = $CLI_W"

# --- 2) deterministic routine fixture (headless dry-run workload) ---
# References real committed pipeline templates; --dry-run never touches a
# device, making the workload fully deterministic.
cat > "$OUT/routine-bench.toml" <<'EOF'
# RSS bench 用 routine (Issue #214 T5 — 決定的ワークロード)。
# 実パイプラインテンプレート (templates/pipelines/*) を参照する検証のみ
# ワークロード (--dry-run はデバイスに触れない)。
name = "bench-daily"

[[steps]]
name = "login"
pipeline_dir = "templates/pipelines/login"
start_task = "LoginTapTitlePc"

[[steps]]
name = "fishing"
pipeline_dir = "templates/pipelines/fishing"
start_task = "FishingStartPc"
EOF
ROUTINE_W="$(cygpath -w "$ROOT/$OUT/routine-bench.toml")"

# --- 3) peak RSS workloads ---
# bash -> powershell.exe argument passing mangles embedded backslashes and
# comma arrays (MSYS layer), so generate a tiny runner .ps1 with the literals
# baked in and invoke that via -File.
run_rss() {
  local label="$1"; shift
  local json="$OUT/rss-$label.json"
  local runner="$OUT/.rss-runner-$label.ps1"
  local argLiterals=()
  local a
  for a in "$@"; do argLiterals+=("'$a'"); done
  local argsArray
  argsArray="$(IFS=,; echo "${argLiterals[*]}")"
  # NOTE: single line on purpose — a backtick line-continuation inside this
  # unquoted heredoc would be eaten by bash as command substitution.
  cat > "$runner" <<EOF
& '$(cygpath -w "$ROOT/scripts/quality/rss_bench.ps1")' -Command '$CLI_W' -Arguments @($argsArray) -WorkingDirectory '$ROOT_W' -Repeat $REPEAT -Label '$label' -JsonOut '$(cygpath -w "$ROOT/$json")'
EOF
  echo "bench: rss workload '$label' x$REPEAT"
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$runner")"
}

run_rss routine-dry-run routine "$ROUTINE_W" --dry-run
run_rss cli-help --help

# --- 4) criterion bench with named baseline ---
if [[ -z "${BENCH_SKIP_CRITERION:-}" ]]; then
  echo "bench: criterion log_snapshot_bench (baseline: $CRITERION_BASELINE, jobs=$JOBS)"
  CARGO_BUILD_JOBS="$JOBS" cargo bench -p anaden-studio --bench log_snapshot_bench -- \
    --save-baseline "$CRITERION_BASELINE"
else
  echo "bench: criterion skipped (BENCH_SKIP_CRITERION set)"
fi

# --- 5) hyperfine reference values (optional, informational) ---
# -N executes via CreateProcess directly: cmd.exe mangles forward slashes in
# the command position ('target/release/...' -> 'elease' is not recognized).
if command -v hyperfine >/dev/null 2>&1; then
  echo "bench: hyperfine reference for routine-dry-run"
  hyperfine -N --warmup 1 --runs 5 \
    --export-json "$OUT/hyperfine-routine.json" \
    "target/release/anaden.exe routine $OUT/routine-bench.toml --dry-run"
else
  echo "bench: hyperfine not installed — skipping reference timing"
fi

# --- 6) summary ---
echo "bench: summary"
node -e '
const fs = require("fs");
const dir = "target/quality/bench";
for (const f of fs.readdirSync(dir).filter((x) => x.endsWith(".json")).sort()) {
  try {
    // strip a possible UTF-8 BOM (PS 5.1 Set-Content artifacts)
    const j = JSON.parse(fs.readFileSync(`${dir}/${f}`, "utf8").replace(/^﻿/, ""));
    if (j.summary && j.summary.peakRssBytesMedian !== undefined) {
      console.log(`  ${f}: median peak RSS ${j.summary.peakRssBytesMedian} bytes, median wall ${j.summary.durationMsMedian} ms`);
    }
  } catch { /* hyperfine schema differs; raw file is the artifact */ }
}
'
echo "bench: done — artifacts under target/quality/bench/ (and target/criterion/)"
