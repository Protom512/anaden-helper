
import { classifyDiffKind } from './gate-diff-kind.js';

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

function toNormalizedSet(files) {
  const set = new Set();
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

export function evaluateTicketPrecheck(declaredFiles, changedFiles, mode = 'strict') {
  const declared = toNormalizedSet(declaredFiles);
  const changed = toNormalizedSet(changedFiles);
  const undeclared = [];
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
  const preImpl = mode === 'pre-implementation';
  const hasMismatch = preImpl ? undeclared.length > 0 : (undeclared.length > 0 || missing.length > 0);
  const fail =
    changed.malformed ||
    (declared.malformed && changed.ordered.length > 0) ||
    (declared.ordered.length === 0 && changed.ordered.length > 0) ||
    (preImpl && declared.ordered.length === 0 && changed.ordered.length > 0) ||
    hasMismatch;
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

export function evaluateIssuePremise(input) {
  const invalid = { verdict: 'FAIL', stale: false, duplicate: false };
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed input (fail-closed: precheck unverifiable, dispatch rejected)' };
  }
  const { issueState, linkedBranchesContainIssue, openPRs, ticketKind, subjectPrNumber } = (input);
  if (typeof issueState !== 'string' || (issueState !== 'open' && issueState !== 'closed')) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed issueState (fail-closed: expected "open"|"closed")' };
  }
  if (typeof linkedBranchesContainIssue !== 'boolean') {
    return { ...invalid, reason: 'issue-premise FAIL — malformed linkedBranchesContainIssue (fail-closed: expected boolean)' };
  }
  if (!Array.isArray(openPRs)) {
    return { ...invalid, reason: 'issue-premise FAIL — malformed openPRs (fail-closed: expected array)' };
  }
  if (ticketKind !== undefined && ticketKind !== 'new-implementation' && ticketKind !== 'continuation') {
    return { ...invalid, reason: 'issue-premise FAIL — malformed ticketKind (fail-closed: expected "new-implementation"|"continuation")' };
  }
  if (subjectPrNumber !== undefined && ticketKind !== 'continuation') {
    return { ...invalid, reason: 'issue-premise FAIL — contradictory input: subjectPrNumber declared without ticketKind "continuation" (fail-closed)' };
  }
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
  if (ticketKind === 'continuation' && subjectPr !== null) {
    const isSubjectPr = ( pr) =>
      pr !== null && typeof pr === 'object' && (pr).number === subjectPr;
    if (!openPRs.some(isSubjectPr)) {
      return {
        verdict: 'FAIL',
        stale: false,
        duplicate,
        reason: `issue-premise FAIL — subject premise broken: declared subject PR #${subjectPr} is not among the ${openPRs.length} open PR(s) referencing this issue (merged or number mismatch)`,
      };
    }
    const remaining = openPRs.filter(( pr) => !isSubjectPr(pr));
    if (remaining.length === 0) {
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

export function deriveSliceMetadata(changedFiles) {
  const changed = toNormalizedSet(changedFiles);
  const changedCrates = changed.ordered
    .filter((f) => f.startsWith('crates/'))
    .map((f) => f.split('/')[1])
    .filter(( c) => typeof c === 'string' && c.length > 0)
    .sort();
  return {
    changedCrates: [...new Set(changedCrates)],
    diffKind: classifyDiffKind(changedFiles),
  };
}
