#!/usr/bin/env bash
# run_review_gate_eval.sh
#
# Issue #76 / Feature: review-gate レビュー精度の定量検証 — 実行再現ツール。
#
# review-gate workflow (.claude/workflows/review-gate.js) は
# `git diff --name-only HEAD` / `git diff --name-only --staged`
# (working tree の変更) を前提としており、マージ済み PR の diff には
# そのまま実行できない。本スクリプトは指定 PR のマージ結果を一時
# worktree (detached, ブランチ無し) にチェックアウトし、base commit へ
# `git reset --soft` することで「PR の変更が staged にある状態」を再現する。
#
# workflow 自体は変更しない。セットアップ完了後に Claude Code を
# worktree 上で起動し review-gate workflow を実行する手順を出力する。
#
# Usage:
#   ./scripts/run_review_gate_eval.sh <pr-number> [<pr-number> ...]
#   ./scripts/run_review_gate_eval.sh --keep <pr-number>   # worktree を残す
#
# Requirements:
#   - git (worktree 対応), gh CLI (PR メタデータ取得)
#   - PR はマージ済みであること (merge commit が origin に存在)
#
# Exit codes:
#   0 = 全 PR のセットアップ成功
#   1 = 前提ツール不足 / PR 解決失敗 / worktree 作成失敗
#
# Cleanup:
#   - デフォルトではスクリプト終了時 (trap EXIT) に worktree を削除する。
#     Claude Code で review-gate を実行する必要があるため、実際の運用では
#     --keep を付けて実行し、評価完了後に表示されるコマンドで削除する。

set -euo pipefail

KEEP=0
PRS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1 ;;
    -h|--help)
      awk 'NR>1 && !/^#/ {exit} NR>1 {print}' "$0"
      exit 0
      ;;
    *)
      if ! [[ "$1" =~ ^[0-9]+$ ]]; then
        echo "ERROR: invalid argument '$1' (expected PR number)" >&2
        exit 1
      fi
      PRS+=("$1")
      ;;
  esac
  shift
done

if [ "${#PRS[@]}" -eq 0 ]; then
  echo "ERROR: at least one PR number is required" >&2
  echo "Usage: $0 [--keep] <pr-number> [<pr-number> ...]" >&2
  exit 1
fi

command -v git >/dev/null 2>&1 || { echo "ERROR: git not found" >&2; exit 1; }
command -v gh  >/dev/null 2>&1 || { echo "ERROR: gh not found" >&2;  exit 1; }

REPO_ROOT="$(git rev-parse --show-toplevel)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 1
}
# shellcheck disable=SC2164
cd "$REPO_ROOT"

# クリーンアップ (git worktree remove のみ。branch は作らないので削除不要)。
CLEANUP_DIRS=()
cleanup() {
  local d
  for d in "${CLEANUP_DIRS[@]:-}"; do
    [ -n "$d" ] && [ -d "$d" ] && git worktree remove --force "$d" >/dev/null 2>&1 || true
  done
  # 孤立した worktree メタデータの掃除 (失敗は無視)
  git worktree prune --expire 1.day >/dev/null 2>&1 || true
}
trap cleanup EXIT

setup_pr() {
  local pr="$1"
  echo "=== PR #${pr} ==="

  local meta base_ref merge_oid
  meta="$(gh pr view "$pr" --json baseRefName,mergeCommit,state 2>/dev/null)" || {
    echo "ERROR: cannot fetch PR #${pr} metadata" >&2
    return 1
  }
  merge_oid="$(printf '%s' "$meta" | sed -n 's/.*"mergeCommit": *{[^}]*"oid": *"\([0-9a-f]*\)".*/\1/p')"
  base_ref="$(printf '%s' "$meta" | sed -n 's/.*"baseRefName": *"\([^"]*\)".*/\1/p')"

  if [ -z "$merge_oid" ] || [ "$merge_oid" = "null" ]; then
    echo "ERROR: PR #${pr} is not merged (no merge commit)" >&2
    return 1
  fi

  # マージコミットは origin (master 等) に到達可能なはず。fetch して確認。
  git fetch --quiet origin "${base_ref}" || git fetch --quiet origin || true
  if ! git cat-file -e "${merge_oid}^{commit}" 2>/dev/null; then
    echo "ERROR: merge commit ${merge_oid} not found locally (fetch first)" >&2
    return 1
  fi

  # base = マージコミットの first parent (master 側の親)
  local base_oid
  base_oid="$(git rev-parse "${merge_oid}^1")"

  local wt
  wt="$(mktemp -d "${TMPDIR:-/tmp}/rg-eval-pr-${pr}.XXXXXX")"
  rmdir "$wt"  # git worktree add が空でないディレクトリを拒否するため

  if ! git worktree add --quiet --detach "$wt" "$merge_oid"; then
    echo "ERROR: failed to create worktree at $wt" >&2
    return 1
  fi
  CLEANUP_DIRS+=("$wt")

  # staged diff = PR の変更、という状態を再現 (HEAD は base のまま)
  git -C "$wt" reset --quiet --soft "$base_oid"

  local files
  files="$(git -C "$wt" diff --name-only --staged)"
  if [ -z "$files" ]; then
    echo "ERROR: staged diff is empty for PR #${pr} (unexpected)" >&2
    return 1
  fi

  echo "worktree    : $wt"
  echo "merge commit: $merge_oid"
  echo "base commit : $base_oid (${base_ref}^)"
  echo "changed files ($(printf '%s\n' "$files" | wc -l | tr -d ' ')):"
  printf '%s\n' "$files" | sed 's/^/  /'
  echo
  echo "Next steps (review-gate 実行):"
  echo "  1. cd $wt"
  echo "  2. claude   # Claude Code を worktree 上で起動"
  echo "  3. review-gate workflow を実行 (3 reviewer 構成は workflow が自動選択)"
  echo "     - 変更が staged にあるため workflow の 'Analyze' phase が"
  echo "       git diff --name-only --staged で PR diff を検出する"
  echo "  4. 評価完了後のクリーンアップ (master への影響なし):"
  echo "     git -C \"$REPO_ROOT\" worktree remove --force $wt"
  echo
}

FAIL=0
for pr in "${PRS[@]}"; do
  setup_pr "$pr" || FAIL=1
done

if [ "$KEEP" -eq 1 ]; then
  # --keep の場合は trap による自動削除を無効化し、利用者の手動削除に委ねる
  echo "--keep specified; worktrees will NOT be removed automatically."
  if [ "${#CLEANUP_DIRS[@]}" -gt 0 ]; then
    printf '%s\n' "${CLEANUP_DIRS[@]}" | sed 's/^/  worktree: /'
  fi
  CLEANUP_DIRS=()
fi

if [ "$FAIL" -ne 0 ]; then
  echo "ERROR: one or more PRs failed to set up" >&2
  exit 1
fi
echo "All worktrees set up successfully."
exit 0
