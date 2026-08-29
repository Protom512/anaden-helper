// Diff-kind classifier + file ownership map / per-lane diff slicing
// (Issue #95 / P-008 T1 + T2).
//
// Pure functions only — no I/O, no git invocation. Used by the Commit Gate
// to short-circuit lane execution for docs-only changes.
//
// Fail-closed principle (UC-3): empty, malformed, or unknown input is
// classified as 'code' so an under-classified diff can never skip review
// lanes. 'docs-only' is only returned when every changed path provably
// matches a documentation pattern.

const DOCS_DIR_PREFIXES = ['docs/', 'doc/'];

const DOCS_ROOT_PREFIXES = ['.claude/rules/', 'CLAUDE.d/'];

/**
 * Whether a single changed-file path is documentation.
 *
 * @param {unknown} path
 * @returns {boolean}
 */
export function isDocsPath(path) {
  if (typeof path !== 'string') {
    return false;
  }
  const p = path.replace(/\\/g, '/').trim();
  if (p.length === 0) {
    return false;
  }
  if (p.toLowerCase().endsWith('.md') || p.toLowerCase().endsWith('.markdown')) {
    return true;
  }
  if (DOCS_DIR_PREFIXES.some((prefix) => p.startsWith(prefix))) {
    return true;
  }
  if (DOCS_ROOT_PREFIXES.some((prefix) => p.startsWith(prefix))) {
    return true;
  }
  return false;
}

/**
 * Classify a list of changed files.
 *
 * @param {unknown} changedFiles
 * @returns {'docs-only' | 'code' | 'mixed'}
 *   - 'docs-only': non-empty input, every entry matches a docs pattern
 *   - 'code':      any non-docs entry present, OR fail-closed default for
 *                  empty / malformed input (never treat unknown as docs)
 *   - 'mixed' is not returned by this classifier; a list containing both
 *     docs and code files is 'code' with docs present. Kept in the return
 *     union for future lane-assignment granularity (see T3 wiring).
 */
export function classifyDiffKind(changedFiles) {
  if (!Array.isArray(changedFiles) || changedFiles.length === 0) {
    return 'code';
  }
  let sawDocs = false;
  let sawCode = false;
  for (const f of changedFiles) {
    if (typeof f !== 'string' || f.trim().length === 0) {
      // Malformed entry is "unclassified" → fail-closed straight to 'code'
      // (UC-3: structurally prevent a false short-circuit).
      return 'code';
    }
    if (isDocsPath(f)) {
      sawDocs = true;
    } else {
      sawCode = true;
    }
  }
  if (!sawCode) {
    return 'docs-only';
  }
  return sawDocs ? 'mixed' : 'code';
}

// ═══════════════════════════════════════════════════════════════════════
// T2: file ownership map + per-lane diff slicing
// ═══════════════════════════════════════════════════════════════════════

/**
 * The 6 gate dimension keys — must mirror GATE_DIMENSIONS in
 * feature-pipeline.js (kept in sync by tests/gate-lane-ownership wiring
 * checks in T3; here asserted against the canonical set).
 */
export const ALL_LANES = /** @type {const} */ ([
  'reliability',
  'performance',
  'extensibility',
  'governance',
  'security',
  'integration',
]);

/** Lanes that review Rust workspace code (crates/**). */
const CRATES_LANES = ['reliability', 'performance', 'extensibility', 'security'];

/** Lanes that review pipeline/tooling scripts (scripts/**, .claude/**). */
const TOOLING_LANES = ['governance', 'integration'];

/**
 * Resolve the owning lanes for a single changed-file path.
 *
 * Fail-closed: unknown, empty, or malformed paths map to ALL_LANES so an
 * unclassifiable file can never skip review lanes (UC-3).
 *
 * @param {unknown} path
 * @returns {readonly string[]}
 */
export function lanesForFile(path) {
  if (typeof path !== 'string') {
    return ALL_LANES;
  }
  const p = path.replace(/\\/g, '/').trim();
  if (p.length === 0) {
    return ALL_LANES;
  }
  if (p.startsWith('crates/')) {
    return CRATES_LANES;
  }
  if (p.startsWith('scripts/') || p.startsWith('.claude/')) {
    return TOOLING_LANES;
  }
  // Documentation files are covered by governance (documentation lane).
  if (isDocsPath(p)) {
    return ['governance'];
  }
  return ALL_LANES;
}

/**
 * Split a unified diff into per-lane slices keyed by lane.
 *
 * Each `diff --git a/X b/X` file section is injected only into the lanes
 * that own path X (ownership 外の diff は注入しない). A requested lane whose
 * ownership covers no changed file receives '' — the T3 wiring treats that
 * as the short-circuit signal (with governance + >=1 code-adjacent lane
 * always kept, per estimate-approval conditions).
 *
 * Fail-closed: files with unknown ownership are injected into every
 * requested lane.
 *
 * @param {unknown} diffText unified diff text
 * @param {readonly string[]} lanes lane keys to build slices for
 * @returns {Record<string, string>} lane -> slice (all requested lanes present)
 */
export function sliceDiffForLanes(diffText, lanes) {
  /** @type {Record<string, string>} */
  const slices = {};
  for (const lane of lanes) {
    slices[lane] = '';
  }
  if (typeof diffText !== 'string' || diffText.length === 0) {
    return slices;
  }
  // Split into per-file sections: each starts at a `diff --git` line.
  const sections = diffText.split(/^(?=diff --git )/m).filter((s) => s.startsWith('diff --git '));
  if (sections.length === 0) {
    // Not a recognizable unified diff → fail-closed to no content per lane
    // (T3 wiring decides via classifyDiffKind on the file list instead).
    return slices;
  }
  for (const section of sections) {
    const m = section.match(/^diff --git a\/(\S+) b\/(\S+)/);
    if (!m) {
      continue;
    }
    // Prefer the "a/" side path (rename detection: b/ side is the new name;
    // owning lanes of the destination are the superset that matters for
    // review, so fall back to b/ when a/ is /dev/null).
    const aPath = m[1];
    const bPath = m[2];
    const path = aPath && aPath !== '/dev/null' ? aPath : bPath;
    const ownerLanes = lanesForFile(path);
    for (const lane of lanes) {
      if (ownerLanes.includes(lane) || ownerLanes === ALL_LANES) {
        slices[lane] += (slices[lane].length > 0 ? '\n' : '') + section.trimEnd();
      }
    }
  }
  return slices;
}
