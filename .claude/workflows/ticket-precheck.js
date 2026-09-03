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
export function evaluateTicketPrecheck(declaredFiles, changedFiles, mode = 'strict') {
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
  // mode='pre-implementation' (Request->Estimate): declared-but-unchanged (missing)
  // is NORMAL (implementation follows declaration). Only undeclared / malformed /
  // empty-declaration FAIL. mode='strict' (default, gate-time): full mismatch FAIL.
  const preImpl = mode === 'pre-implementation';
  const hasMismatch = preImpl ? undeclared.length > 0 : (undeclared.length > 0 || missing.length > 0);
  // Fail-closed: changed-files side malformed is always FAIL; declared side
  // malformed is FAIL only when there is actual diff content to check
  // against (both-empty is the only clean vacuous case).
  const fail =
    changed.malformed ||
    (declared.malformed && changed.ordered.length > 0) ||
    (declared.ordered.length === 0 && changed.ordered.length > 0) ||
    // Issue #131 修正: pre-implementation で changed も空なら「検証のみチケット」として
    // PASS (空宣言×非空diff のみ FAIL — fail-closed は維持)。
    (preImpl && declared.ordered.length === 0 && changed.ordered.length > 0) ||
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
    reason: fail ? `ticket-precheck FAIL (${mode}) — ${parts.join('; ')}` : (preImpl && missing.length > 0 ? `ticket-precheck PASS (pre-implementation) — ${missing.length} declared file(s) pending implementation; no undeclared changed files` : 'ticket-precheck PASS — declared files match changed files'),
  };
}

/**
 * Issue premise verification (Issue #109 Task 1): detect stale / duplicate
 * dispatch targets BEFORE Request/Estimate proceeds.
 *
 * Input fields (all supplied by the wiring layer — pure fn does no I/O):
 *   - issueState: 'open' | 'closed' (from `gh issue view --json state`)
 *   - linkedBranchesContainIssue: boolean — any trunk-reachable branch
 *     contains the issue's implementation commit(s) (from
 *     `git branch -a --contains <sha>`; wiring resolves commit SHAs)
 *   - openPRs: array of open PR objects for the same issueNumber
 *     (from `gh pr list --search "<n> in:body" --json number,title`)
 *   - ticketKind (optional, Issue #150): 'new-implementation' | 'continuation'.
 *     Absent = legacy behavior (no exemption). Present-but-invalid -> FAIL.
 *   - subjectPrNumber (optional, Issue #150): positive integer PR number or
 *     /^\d+$/ numeric string (normalized to number). Only meaningful with
 *     ticketKind='continuation' — declaring it otherwise is contradictory
 *     input -> FAIL.
 *
 * Verdicts:
 *   - stale:     issue closed AND merged into trunk -> FAIL
 *     (priority over everything — the Issue #150 exemption applies to
 *     duplicate only, never to stale)
 *   - duplicate: any open PR referencing the same issue -> FAIL, EXCEPT the
 *     Issue #150 continuation exemption: ticketKind='continuation' + valid
 *     subjectPrNumber + the declared subject PR is among openPRs + no
 *     unrelated open PRs remain -> PASS (reason cites the subject PR number).
 *     A continuation ticket must not be self-blocked by its own subject PR.
 *     Subject declared but absent from openPRs (merged / mismatch) -> FAIL
 *     (premise broken — openPRs empty included).
 *   - closed-but-unmerged is PASS (closed != merged; e.g. wontfix-reopened)
 *
 * Fail-closed: null / non-object input, missing fields, or invalid field
 * types (gh auth failure / rate limit surface as malformed input) -> FAIL.
 * Never fail-open — a fail-open would let stale detection slip through.
 *
 * NOTE: this pure function must stay free of external references (only
 * standard JS builtins) — ticket-precheck-drift.test.mjs evals the verbatim
 * inline copy via `new Function` with only classifyDiffKind injected.
 *
 * @param {unknown} input
 * @returns {{ verdict: 'PASS'|'FAIL', reason: string, stale: boolean,
 *   duplicate: boolean }}
 */
export function evaluateIssuePremise(input) {
  const invalid = { verdict: 'FAIL', stale: false, duplicate: false };
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed input (fail-closed: precheck unverifiable, dispatch rejected)' };
  }
  const { issueState, linkedBranchesContainIssue, openPRs, ticketKind, subjectPrNumber } = /** @type {Record<string, unknown>} */ (input);
  if (typeof issueState !== 'string' || (issueState !== 'open' && issueState !== 'closed')) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed issueState (fail-closed: expected "open"|"closed")' };
  }
  if (typeof linkedBranchesContainIssue !== 'boolean') {
    return { ...invalid, reason: 'issue-premise FAIL — malformed linkedBranchesContainIssue (fail-closed: expected boolean)' };
  }
  if (!Array.isArray(openPRs)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed openPRs (fail-closed: expected array)' };
  }
  // Issue #150: optional ticketKind — absent (undefined) = legacy behavior;
  // present-but-invalid (incl. null) is fail-closed FAIL, never ignored.
  if (ticketKind !== undefined && ticketKind !== 'new-implementation' && ticketKind !== 'continuation') {
    return { ...invalid, reason: 'issue-premise FAIL — malformed ticketKind (fail-closed: expected "new-implementation"|"continuation")' };
  }
  // subjectPrNumber is only meaningful for continuation tickets — declaring it
  // alongside new-implementation (or without any ticketKind) is contradictory.
  if (subjectPrNumber !== undefined && ticketKind !== 'continuation') {
    return { ...invalid, reason: 'issue-premise FAIL — contradictory input: subjectPrNumber declared without ticketKind "continuation" (fail-closed)' };
  }
  // Normalize subjectPrNumber: positive integer, or /^\d+$/ numeric string
  // converted to number ("149" -> 149). Zero / negative / non-numeric FAIL.
  let subjectPr = null;
  if (subjectPrNumber !== undefined) {
    let n = null;
    if (typeof subjectPrNumber === 'number') {
      n = subjectPrNumber;
    } else if (typeof subjectPrNumber === 'string' && /^\d+$/.test(subjectPrNumber)) {
      n = Number(subjectPrNumber);
    }
    if (n === null || !Number.isInteger(n) || n <= 0) {
      return { ...invalid, reason: 'issue-premise FAIL — malformed subjectPrNumber (fail-closed: expected positive integer PR number)' };
    }
    subjectPr = n;
  }
  const stale = issueState === 'closed' && linkedBranchesContainIssue;
  const duplicate = openPRs.length > 0;
  if (stale) {
    return {
      verdict: 'FAIL',
      stale: true,
      duplicate,
      reason: duplicate
        ? 'issue-premise FAIL — stale: issue is closed and already merged into trunk; duplicate: open PR(s) also exist'
        : 'issue-premise FAIL — stale: issue is closed and already merged into trunk',
    };
  }
  // Issue #150 continuation duplicate exemption. Only reached when not stale
  // (stale keeps priority — the exemption targets duplicate exclusively).
  if (ticketKind === 'continuation' && subjectPr !== null) {
    const isSubjectPr = (/** @type {unknown} */ pr) =>
      pr !== null && typeof pr === 'object' && /** @type {Record<string, unknown>} */ (pr).number === subjectPr;
    if (!openPRs.some(isSubjectPr)) {
      // Declared subject PR is not open (merged / number mismatch): the
      // continuation premise itself is broken -> FAIL even when openPRs=[].
      return {
        verdict: 'FAIL',
        stale: false,
        duplicate,
        reason: `issue-premise FAIL — subject premise broken: declared subject PR #${subjectPr} is not among the ${openPRs.length} open PR(s) referencing this issue (merged or number mismatch)`,
      };
    }
    const remaining = openPRs.filter((/** @type {unknown} */ pr) => !isSubjectPr(pr));
    if (remaining.length === 0) {
      // Exemption: every open PR referencing this issue IS the declared
      // subject — the continuation ticket is not a duplicate of itself.
      return {
        verdict: 'PASS',
        stale: false,
        duplicate,
        reason: `issue-premise PASS — continuation exemption (Issue #150): all ${openPRs.length} open PR(s) referencing this issue are the declared subject PR #${subjectPr}; no unrelated duplicate PRs`,
      };
    }
    return {
      verdict: 'FAIL',
      stale: false,
      duplicate: true,
      reason: `issue-premise FAIL — duplicate: ${remaining.length} unrelated open PR(s) still reference this issue after excluding declared subject PR #${subjectPr}`,
    };
  }
  if (duplicate) {
    return {
      verdict: 'FAIL',
      stale: false,
      duplicate: true,
      reason: `issue-premise FAIL — duplicate: ${openPRs.length} open PR(s) already reference this issue`,
    };
  }
  return {
    verdict: 'PASS',
    stale: false,
    duplicate: false,
    reason: `issue-premise PASS — issue is ${issueState}, not merged into trunk, no open duplicate PRs`,
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
