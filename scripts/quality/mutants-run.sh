#!/usr/bin/env bash
# cargo-mutants 長時間ランの自動復旧ランナー (Issue #228 / #221 / #242)。
#
# 実測された障害: 3201 mutants のワークスペースランが数時間ごとに nextest
# クラッシュ (exit -1073741205 = 0xC0000409、メモリ/コミット上限系) で中断し、
# --in-place の変異ソースが crates/ 全体に残留する (3 回観測)。
#
# 本ランナーは:
#   1. cargo mutants --workspace --in-place --jobserver-tasks 4 --iterate を実行
#      (--iterate は前回 caught 済み mutant をスキップ → クラッシュ再開で前進保証)
#   2. 異常終了 (rc>1) 時は変異ソース・テスト残骸を復元してリトライ (上限 RETRY_MAX)
#   3. 正常終了 (rc 0=全 caught / 1=生存あり) で mutants_gate check と
#      mutants_reconcile を実行して完了
#   4. 終了コードは gate/reconcile の結果を伝播 (Issue #242 — 従来常に exit 0)
#
# 使い方 (無人実行):
#   nohup ./scripts/quality/mutants-run.sh <baseline.json> > mutants-run.log 2>&1 &
set -u
cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)" || exit 1
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
RETRY_MAX="${RETRY_MAX:-20}"
BASELINE="${1:-target/quality/mutants-baseline.json}"

# Issue #233: クラッシュ復旧の cleanup_tree が git checkout -- crates で
# 正当な未コミット変更を破壊しないよう、開始時に crates/ が clean であることを要求。
if [ -n "$(git status --porcelain crates 2>/dev/null)" ]; then
  echo "ERROR: crates/ has uncommitted changes — commit or stash before running." >&2
  git status --porcelain crates | head -5 >&2
  exit 1
fi

cleanup_tree() {
  # --in-place 変異ソース残留の復元 + テスト残骸 (engine テストが書く PNG) の削除。
  # Issue #242: git checkout の失敗を検知してリトライを中止 (変異ソースの上から
  # 再開するのを防ぐ)。AV / index.lock / Windows ファイルロックで失敗し得る。
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

# Issue #242: 終了コードを正しく伝播する。
final_rc=0

# ゲート判定 (baseline が必要なのは check のみ)。
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
