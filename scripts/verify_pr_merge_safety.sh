#!/usr/bin/env bash

set -u

if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git not found in PATH" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 2
}
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
  if out="$(git merge-tree --write-tree --merge-base="${BASE_SHA}" "${A_SHA}" "${B_SHA}" 2>&1)"; then
    echo "RESULT: merge SAFE (no conflicts) via merge-tree --write-tree"
    exit 0
  else
    echo "RESULT: CONFLICT detected via merge-tree --write-tree" >&2
    echo "$out" >&2
    exit 1
  fi
fi

out="$(git merge-tree "${BASE_SHA}" "${A_SHA}" "${B_SHA}" 2>&1)"
if echo "$out" | grep -qE '(^|\+)(<<<<<<<|>>>>>>>|=======)|CONFLICT'; then
  echo "RESULT: CONFLICT detected via legacy merge-tree" >&2
  echo "$out" >&2
  exit 1
fi
echo "RESULT: merge SAFE (no conflicts) via legacy merge-tree"
exit 0
