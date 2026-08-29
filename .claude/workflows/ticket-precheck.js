// Ticket precheck pure functions (Issue #99 Task 2).
//
// Verifies that ticket-declared files match the actually-changed files
// (working-tree + untracked + commit-range fallback — the caller supplies the
// combined list) BEFORE the pipeline reaches the gate. Eliminates
// self-declared slice metadata by deriving crates/diff-kind from the changed
// files themselves.
//
// Pure functions only — no I/O, no git invocation. Diff-kind classification
// is REUSED from gate-diff-kind.js (estimate-approval condition: no second
// drift surface).
//
// Fail-closed principle: any mismatch, malformed input, or undeclared changed
// file is a FAIL. A non-empty diff with an empty declaration is a FAIL.

import { classifyDiffKind } from './gate-diff-kind.js';

/**
 * Normalize a file path: unify separators to '/', strip './' prefix,
 * collapse duplicate slashes, trim whitespace.
 *
 * @param {unknown} path
 * @returns {string | null} normalized path, or null for malformed input
 */
function normalizePath(path) {
  if (typeof path !== 'string') {
    return null;
  }
  let p = path.replace(/\\/g, '/').trim();
  while (p.startsWith('./')) {
    p = p.slice(2);
  }
  p = p.replace(/\/{2,}/g, '/');
  return p.length > 0 ? p : null;
}

/**
 * Build a normalized de-duplicated path set from a file list.
 * Malformed entries (non-string, blank) are collected as `malformed: true`.
 *
 * @param {unknown} files
 * @returns {{ set: Set<string>, ordered: string[], malformed: boolean }}
 */
function toNormalizedSet(files) {
  /** @type {Set<string>} */
  const set = new Set();
  /** @type {string[]} */
  const ordered = [];
  let malformed = false;
  if (!Array.isArray(files)) {
    return { set, ordered, malformed: true };
  }
  for (const f of files) {
    const p = normalizePath(f);
    if (p === null) {
      malformed = true;
      continue;
    }
    if (!set.has(p)) {
      set.add(p);
      ordered.push(p);
    }
  }
  return { set, ordered, malformed };
}

/**
 * Compare ticket-declared files with actually-changed files.
 *
 * UC-1: exact (normalized, order-independent) match -> PASS.
 * UC-2: any mismatch -> FAIL with explicit file lists:
 *   - undeclared: changed but not declared (fail-closed: never silently allow)
 *   - missing:    declared but not changed (stale declaration)
 * Fail-closed edges:
 *   - non-empty diff with empty/malformed declaration -> FAIL
 *   - null / non-array / malformed input -> FAIL
 *   - both empty -> PASS (vacuous-clean; nothing to verify)
 *
 * @param {unknown} declaredFiles ticket-declared file list
 * @param {unknown} changedFiles combined working-tree + untracked +
 *   commit-range-fallback changed file list
 * @returns {{ verdict: 'PASS'|'FAIL', declared: string[], changed: string[],
 *   undeclared: string[], missing: string[], reason: string }}
 */
export function evaluateTicketPrecheck(declaredFiles, changedFiles) {
  const declared = toNormalizedSet(declaredFiles);
  const changed = toNormalizedSet(changedFiles);
  /** @type {string[]} */
  const undeclared = [];
  /** @type {string[]} */
  const missing = [];
  for (const p of changed.ordered) {
    if (!declared.set.has(p)) {
      undeclared.push(p);
    }
  }
  for (const p of declared.ordered) {
    if (!changed.set.has(p)) {
      missing.push(p);
    }
  }
  const hasMismatch = undeclared.length > 0 || missing.length > 0;
  // Fail-closed: changed-files side malformed is always FAIL; declared side
  // malformed is FAIL only when there is actual diff content to check
  // against (both-empty is the only clean vacuous case).
  const fail =
    changed.malformed ||
    (declared.malformed && changed.ordered.length > 0) ||
    (declared.ordered.length === 0 && changed.ordered.length > 0) ||
    hasMismatch;
  /** @type {string[]} */
  const parts = [];
  if (undeclared.length > 0) {
    parts.push(`undeclared changed files: ${undeclared.join(', ')}`);
  }
  if (missing.length > 0) {
    parts.push(`declared but unchanged files: ${missing.join(', ')}`);
  }
  if (declared.ordered.length === 0 && changed.ordered.length > 0) {
    parts.push('non-empty diff with empty declaration');
  }
  if (changed.malformed) {
    parts.push('malformed changed-files input (fail-closed)');
  }
  if (declared.malformed && changed.ordered.length > 0) {
    parts.push('malformed declaration (fail-closed)');
  }
  return {
    verdict: fail ? 'FAIL' : 'PASS',
    declared: declared.ordered,
    changed: changed.ordered,
    undeclared,
    missing,
    reason: fail ? `ticket-precheck FAIL — ${parts.join('; ')}` : 'ticket-precheck PASS — declared files match changed files',
  };
}

/**
 * Derive slice metadata (changed crates + diff-kind) from the actual changed
 * files — replaces self-declared slice metadata. Crates derivation mirrors
 * the existing feature-pipeline.js changedCrates logic (crates/<name>/** ->
 * <name>); diff-kind classification is reused from gate-diff-kind.js.
 *
 * Fail-closed: malformed input -> no crates + diffKind 'code'.
 *
 * @param {unknown} changedFiles
 * @returns {{ changedCrates: string[], diffKind: 'docs-only'|'code'|'mixed' }}
 */
export function deriveSliceMetadata(changedFiles) {
  const changed = toNormalizedSet(changedFiles);
  const changedCrates = changed.ordered
    .filter((f) => f.startsWith('crates/'))
    .map((f) => f.split('/')[1])
    .filter((/** @type {string|undefined} */ c) => typeof c === 'string' && c.length > 0)
    .sort();
  return {
    changedCrates: [...new Set(changedCrates)],
    diffKind: classifyDiffKind(changedFiles),
  };
}
