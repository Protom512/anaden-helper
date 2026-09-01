// Diff-injection helpers for review-gate (Issue #78 Task 2).
// Ports the R8 diff-inject pattern from feature-pipeline.js (L589-635) so that
// reviewer prompts carry the actual working-tree diff instead of only a file list.
// Cap values are kept identical to feature-pipeline.js: 28000 (DIFF threshold
// inside the fetch prompt) / 24000 (truncated DIFF body) / 30000 (total slice).

export const DIFF_TRUNCATE_THRESHOLD = 28000;
export const DIFF_BODY_CAP = 24000;
export const TOTAL_DIFF_CAP = 30000;

export const DIFF_FETCH_PROMPT = `Working-tree diff fetcher for the Review Gate. In the repo CWD run these and return ONE plain-text string (no JSON, no commentary wrapper):
1. \`git --no-pager diff HEAD --stat\`
2. \`git --no-pager diff HEAD\`
3. \`git --no-pager status --porcelain\`
Concatenate with headers "=== STAT ===", "=== DIFF ===", "=== UNTRACKED ===". If the DIFF section exceeds ${DIFF_TRUNCATE_THRESHOLD} chars, emit full STAT + UNTRACKED but only the FIRST ${DIFF_BODY_CAP} chars of DIFF, then a line "[DIFF TRUNCATED]". Return ONLY the concatenated text.`;

/**
 * Normalize the agent's diff-fetch result into a string, capped at TOTAL_DIFF_CAP.
 * Fail-open: null/undefined → '' (gate stays runnable without diff context).
 */
export function normalizeGateDiff(result) {
  const text = typeof result === 'string'
    ? result
    : (result == null ? '' : JSON.stringify(result));
  return text.slice(0, TOTAL_DIFF_CAP);
}

/**
 * Wrap a normalized diff body in === WORKING-TREE DIFF === markers.
 * Returns '' when diff is empty (nothing to inject).
 */
export function buildDiffSection(diff) {
  if (!diff) return '';
  return `=== WORKING-TREE DIFF ===
${diff}
=== END DIFF ===`;
}

/**
 * Append the R8 instruction + diff section to a reviewer prompt.
 * Reviewers must analyze the provided diff directly and NOT re-Read files.
 * Mirrors feature-pipeline.js FEEDBACK_INSTRUCTION (L629-635).
 */
// ---------------------------------------------------------------------------
// Issue #91 Task T1 — commit-range-diff fallback (S2-FP-1 fix proposal).
// Pure logic: prefer working-tree diff when non-empty, else commit-range
// (HEAD~1..HEAD by default, merge-base variant configurable), else fail-closed.
// Evidence payload carries the tree-hash capture (git write-tree) so that
// "green but actually empty" states are detectable (pipeline-evidence rule §2).
// ---------------------------------------------------------------------------

/** Commit-range spec per variant. */
export const RANGE_VARIANTS = {
  'head-prev': 'HEAD~1..HEAD',
  'merge-base': 'origin/master...HEAD',
};

/**
 * Build the gate diff input with working-tree → commit-range → fail-closed
 * fallback (S2-FP-1). Pure function: no git invocation here; callers pass
 * pre-collected strings (stat/diff from `git diff HEAD`, rangeStat/rangeDiff
 * from the commit-range variant, treeHash from `git write-tree`).
 *
 * @param {{stat?:string, diff?:string, untracked?:string,
 *          rangeStat?:string, rangeDiff?:string, treeHash?:string|null}} input
 * @param {{rangeVariant?:'head-prev'|'merge-base'}} [options]
 * @returns {{mode:'working-tree'|'commit-range'|'fail-closed',
 *            stat:string, diff:string, untracked:string,
 *            treeHash?:string, rangeVariant?:string,
 *            reason?:string, note?:string}}
 */
export function buildCommitRangeDiffInput(input, options = {}) {
  const str = (v) => (typeof v === 'string' ? v : '');
  const stat = str(input.stat);
  const diff = str(input.diff);
  const untracked = str(input.untracked);
  const rangeStat = str(input.rangeStat);
  const rangeDiff = str(input.rangeDiff);
  const rangeVariant = options.rangeVariant ?? 'head-prev';

  const out = { stat, diff, untracked };
  if (input.treeHash != null) out.treeHash = input.treeHash;

  if (diff.trim() !== '' || stat.trim() !== '') {
    out.mode = 'working-tree';
    return out;
  }
  if (rangeDiff.trim() !== '' || rangeStat.trim() !== '') {
    out.mode = 'commit-range';
    out.stat = rangeStat;
    out.diff = rangeDiff;
    out.rangeVariant = rangeVariant;
    return out;
  }
  out.mode = 'fail-closed';
  out.reason = 'working-tree diff and commit-range diff are both empty';
  if (untracked.trim() !== '') {
    out.note = 'untracked files present but no tracked diff: gate must not emit a vacuous GO; '
      + 're-run with `git add -N` (intent-to-add) diff or enumerate files for individual Read';
  }
  return out;
}

/**
 * Extract the body of a "=== NAME ===" delimited section from the concatenated
 * plain-text result of a diff-fetch agent (T2, Issue #91). Returns '' when the
 * section is missing or whitespace-only. Used to split the single fetch result
 * into working-tree STAT/DIFF/UNTRACKED, COMMIT-RANGE STAT/DIFF and TREE HASH
 * parts before feeding buildCommitRangeDiffInput.
 */
export function extractDiffSection(raw, name) {
  if (typeof raw !== 'string' || raw === '') return '';
  const startMarker = `=== ${name} ===`;
  const start = raw.indexOf(startMarker);
  if (start < 0) return '';
  const bodyStart = start + startMarker.length;
  const next = raw.indexOf('===', bodyStart);
  const body = next < 0 ? raw.slice(bodyStart) : raw.slice(bodyStart, next);
  return body.trim();
}

// ---------------------------------------------------------------------------
// Issue #102 T1 — unified gate-diff single source of truth (S2 codify).
// Pure function expressing the full deterministic fallback chain of UC-1:
//   (a) working-tree STAT/DIFF non-empty → 'working-tree'
//   (b) empty → HEAD~1..HEAD commit-range
//   (c) still empty → merge-base (origin/master...HEAD)
//   (d) untracked files only → 'intent-to-add' (file list + individual Read)
//   (e) all empty / 429-placeholder input → 'fail-closed'
// Returns basis (mode-decision evidence), treeHash and a pre-assembled
// snapshot string for injection into every gate lane.
// buildCommitRangeDiffInput is reused as the lower-level routine for the
// working-tree vs single-commit-range decision (no breaking change).
// ---------------------------------------------------------------------------

/**
 * Exact-match 429/placeholder sentinels. Detection is exact (whole trimmed
 * section equals one of these) so legitimate diffs containing these words as
 * substrings are NOT fail-closed (estimate approval condition #1).
 */
export const PLACEHOLDER_SENTINELS = ['429', 'rate limit', 'placeholder'];

function isPlaceholderSection(text) {
  const t = (text ?? '').trim();
  return PLACEHOLDER_SENTINELS.includes(t);
}

function assembleSnapshot(stat, diff, header) {
  const parts = [];
  if (stat.trim() !== '') parts.push(`=== STAT ===\n${stat.trim()}`);
  if (diff.trim() !== '') parts.push(`=== DIFF ===\n${diff.trim()}`);
  if (parts.length === 0) return '';
  return (header ? `${header}\n` : '') + parts.join('\n');
}

/**
 * Parse `git status --porcelain` untracked lines into file paths.
 */
function parseUntrackedFiles(untracked) {
  return untracked
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.startsWith('??'))
    .map((l) => l.replace(/^\?\?\s+/, '').replace(/"(.*)"$/, '$1'))
    .filter((l) => l !== '');
}

/**
 * Unified deterministic gate-diff resolver (Issue #102 UC-1). Pure function:
 * callers pass pre-collected git outputs; no git invocation here.
 *
 * @param {{stat?:string, diff?:string, untracked?:string,
 *          headPrevStat?:string, headPrevDiff?:string,
 *          mergeBaseStat?:string, mergeBaseDiff?:string,
 *          treeHash?:string|null}} input
 * @returns {{mode:'working-tree'|'commit-range'|'intent-to-add'|'fail-closed',
 *            basis:string, snapshot:string,
 *            stat?:string, diff?:string, untracked?:string,
 *            treeHash?:string, rangeVariant?:string,
 *            untrackedFiles?:string[], reason?:string, note?:string}}
 */
export function buildUnifiedGateDiff(input) {
  const str = (v) => (typeof v === 'string' ? v : '');
  const fail = (basis, reason) => ({
    mode: 'fail-closed', basis, reason, snapshot: '',
  });

  if (input == null || typeof input !== 'object') {
    return fail('all-empty', 'no gate diff input provided (null/undefined)');
  }

  const stat = str(input.stat);
  const diff = str(input.diff);

  // (e) 429-placeholder detection — EXACT match only (see PLACEHOLDER_SENTINELS).
  if (isPlaceholderSection(stat) || isPlaceholderSection(diff)) {
    return fail('429-placeholder',
      'gate diff input is a 429/rate-limit placeholder string; must fail-closed instead of vacuous GO');
  }

  // (a) working-tree mode — reuses buildCommitRangeDiffInput as the lower
  // routine for the working-tree vs HEAD~1..HEAD decision.
  const headPrev = buildCommitRangeDiffInput({
    stat, diff, untracked: str(input.untracked),
    rangeStat: str(input.headPrevStat),
    rangeDiff: str(input.headPrevDiff),
    treeHash: input.treeHash ?? undefined,
  });

  if (headPrev.mode === 'working-tree') {
    return {
      ...headPrev,
      basis: 'working-tree',
      snapshot: assembleSnapshot(stat, diff, null),
    };
  }

  if (headPrev.mode === 'commit-range') {
    return {
      ...headPrev,
      mode: 'commit-range',
      basis: 'commit-range:HEAD~1..HEAD',
      snapshot: assembleSnapshot(headPrev.stat, headPrev.diff,
        '=== COMMIT-RANGE DIFF (HEAD~1..HEAD) ==='),
    };
  }

  // (c) merge-base fallback (origin/master...HEAD).
  const mergeStat = str(input.mergeBaseStat);
  const mergeDiff = str(input.mergeBaseDiff);
  if (isPlaceholderSection(mergeStat) || isPlaceholderSection(mergeDiff)) {
    return fail('429-placeholder',
      'gate diff input is a 429/rate-limit placeholder string; must fail-closed instead of vacuous GO');
  }
  if (mergeDiff.trim() !== '' || mergeStat.trim() !== '') {
    const out = {
      mode: 'commit-range',
      basis: 'commit-range:origin/master...HEAD',
      stat: mergeStat,
      diff: mergeDiff,
      rangeVariant: 'merge-base',
      snapshot: assembleSnapshot(mergeStat, mergeDiff,
        '=== COMMIT-RANGE DIFF (origin/master...HEAD) ==='),
    };
    if (input.treeHash != null) out.treeHash = input.treeHash;
    return out;
  }

  // (d) untracked-only → intent-to-add mode (file list + individual Read).
  const untracked = str(input.untracked);
  if (untracked.trim() !== '') {
    const files = parseUntrackedFiles(untracked);
    return {
      mode: 'intent-to-add',
      basis: 'untracked-only',
      untracked,
      untrackedFiles: files,
      reason: 'no tracked diff; untracked files present',
      note: 'gate must run `git add -N` (intent-to-add) or have reviewers Read each file individually; never emit a vacuous GO',
      snapshot: files.length > 0
        ? `=== UNTRACKED FILES (intent-to-add; Read each file individually) ===\n${files.join('\n')}`
        : untracked,
      ...(input.treeHash != null ? { treeHash: input.treeHash } : {}),
    };
  }

  // (e) everything empty → fail-closed.
  return fail('all-empty',
    'working-tree, HEAD~1..HEAD and merge-base diffs are all empty and no untracked files exist');
}

export function withDiffContext(prompt, diffSection) {
  if (!diffSection) return prompt;
  return `${prompt}

【提供済み diff で直接分析（R8 — diff-inject, Issue #78）】下記に working-tree の diff を示す。
これを直接読んで分析すること。ファイルを個別に Read して再取得*しない*こと（diff が手元にある）。
あなたの役割に特有の grep/check は1回まで許可するが、tool 使用は最小限にし、最終アクションは
*必ず* StructuredOutput を呼ぶこと（prose-only で終わらせない）。findings の file/line は
この diff の hunk に基づくこと（working tree の現状態ではなく）。

${diffSection}`;
}
