#!/usr/bin/env bash
# verify_pr_merge_safety.sh
#
# Task 4 (downgraded to a verification gate per estimate approval conditions):
# PR #15 (feat/pc-title-pc-roi-contract-test) と PR #16
# (feat/field-loop-pc-roi-derivation) は両方とも
# crates/anaden-vision/src/pipeline.rs を変更するが、変更 hunk は
# 行位置的に disjoint (PR#15: 1445/1912, PR#16: 1684 in base coordinates)
# であるため、git の 3-way merge は機械的コンフリクト無しで適用できる。
#
# 本スクリプトはその事実を merge-tree dry-run で検証し、コンフリクトが
# 発生する場合は非 zero で終了する。第2 PR マージ前に本スクリプトを
# 実行して exit 0 を確認すること。コンフリクトが報告された場合のみ
# 手動 rebase / master マージでの解消へエスカレーションする。
#
# Usage:
#   ./scripts/verify_pr_merge_safety.sh
#   ./scripts/verify_pr_merge_safety.sh <branch-a> <branch-b> [<merge-base>]
#
# Exit codes:
#   0 = マージ安全（コンフリクト無し）
#   1 = コンフリクト検出、または git 未対応
#   2 = 引数/ブランチ解決エラー

set -u

if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git not found in PATH" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 2
}
# shellcheck disable=SC2164
cd "$REPO_ROOT"

BRANCH_A="${1:-feat/pc-title-pc-roi-contract-test}"
BRANCH_B="${2:-feat/field-loop-pc-roi-derivation}"
EXPLICIT_BASE="${3:-}"

resolve_ref() {
  local ref="$1"
  local resolved
  resolved="$(git rev-parse --verify "${ref}^{commit}" 2>/dev/null)" || {
    echo "ERROR: cannot resolve ref '${ref}' to a commit" >&2
    exit 2
  }
  printf '%s' "$resolved"
}

A_SHA="$(resolve_ref "$BRANCH_A")"
B_SHA="$(resolve_ref "$BRANCH_B")"

if [ -n "$EXPLICIT_BASE" ]; then
  BASE_SHA="$(resolve_ref "$EXPLICIT_BASE")"
else
  BASE_SHA="$(git merge-base "$A_SHA" "$B_SHA" 2>/dev/null)" || {
    echo "ERROR: no merge-base between ${BRANCH_A} and ${BRANCH_B}" >&2
    exit 2
  }
fi

echo "branch A     : ${BRANCH_A} (${A_SHA})"
echo "branch B     : ${BRANCH_B} (${B_SHA})"
echo "merge-base   : ${BASE_SHA}"

# Git 2.38+ の --write-tree 形式かを実際にコマンドで判定する
# (help テキストの grep はフォーマット差で誤判定するため)。
# 注意: --write-tree で「同一コミット同士」のマージは Git によっては
# fatal(exits 128) になるため、ベースと子2つという有効な自明マージで検証する。
supports_write_tree() {
  local probe base c1 c2
  probe="$(mktemp -d)"
  git init -q "$probe"
  git -C "$probe" -c user.email=t@t -c user.name=t commit -q --allow-empty -m base
  base="$(git -C "$probe" rev-parse HEAD)"
  git -C "$probe" checkout -q -b c1
  git -C "$probe" -c user.email=t@t -c user.name=t commit -q --allow-empty -m c1
  c1="$(git -C "$probe" rev-parse HEAD)"
  git -C "$probe" checkout -q "$base"
  git -C "$probe" checkout -q -b c2
  git -C "$probe" -c user.email=t@t -c user.name=t commit -q --allow-empty -m c2
  c2="$(git -C "$probe" rev-parse HEAD)"
  git merge-tree --write-tree --merge-base="$base" "$c1" "$c2" >/dev/null 2>&1
  local rc
  rc=$?
  rm -rf "$probe"
  return "$rc"
}

if supports_write_tree; then
  # --write-tree はコンフリクト時に exit 1 + コンフリクトパス/CONFLICT 行を出力。
  if out="$(git merge-tree --write-tree --merge-base="${BASE_SHA}" "${A_SHA}" "${B_SHA}" 2>&1)"; then
    echo "RESULT: merge SAFE (no conflicts) via merge-tree --write-tree"
    exit 0
  else
    echo "RESULT: CONFLICT detected via merge-tree --write-tree" >&2
    echo "$out" >&2
    exit 1
  fi
fi

# フォールバック: 旧式 merge-tree <base> <a> <b>。
# 注意: "changed in both" / "added in both" は「両ブランチが同一ファイルを変更した」
# というだけの正常エントリ（hunk が disjoint なら自動マージで解決する）。これを
# コンフリクト判定に使うと偽陽性になる。真のコンンフリクト指標は diff 内の
# コンフリクトマーカ (<<<<<<< .our / ======= / >>>>>>> .their, 先頭 + 付き) のみ。
out="$(git merge-tree "${BASE_SHA}" "${A_SHA}" "${B_SHA}" 2>&1)"
if echo "$out" | grep -qE '(^|\+)(<<<<<<<|>>>>>>>|=======)|CONFLICT'; then
  echo "RESULT: CONFLICT detected via legacy merge-tree" >&2
  echo "$out" >&2
  exit 1
fi
echo "RESULT: merge SAFE (no conflicts) via legacy merge-tree"
exit 0
