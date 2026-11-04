
export const DIFF_TRUNCATE_THRESHOLD = 28000;
export const DIFF_BODY_CAP = 24000;
export const TOTAL_DIFF_CAP = 30000;

export const DIFF_FETCH_PROMPT = `Working-tree diff fetcher for the Review Gate. In the repo CWD run these and return ONE plain-text string (no JSON, no commentary wrapper):
1. \`git --no-pager diff HEAD --stat\`
2. \`git --no-pager diff HEAD\`
3. \`git --no-pager status --porcelain\`
Concatenate with headers "=== STAT ===", "=== DIFF ===", "=== UNTRACKED ===". If the DIFF section exceeds ${DIFF_TRUNCATE_THRESHOLD} chars, emit full STAT + UNTRACKED but only the FIRST ${DIFF_BODY_CAP} chars of DIFF, then a line "[DIFF TRUNCATED]". Return ONLY the concatenated text.`;

export function normalizeGateDiff(result) {
  const text = typeof result === 'string'
    ? result
    : (result == null ? '' : JSON.stringify(result));
  return text.slice(0, TOTAL_DIFF_CAP);
}

export function buildDiffSection(diff) {
  if (!diff) return '';
  return `=== WORKING-TREE DIFF ===
${diff}
=== END DIFF ===`;
}

export const RANGE_VARIANTS = {
  'head-prev': 'HEAD~1..HEAD',
  'merge-base': 'origin/master...HEAD',
};

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

function parseUntrackedFiles(untracked) {
  return untracked
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.startsWith('??'))
    .map((l) => l.replace(/^\?\?\s+/, '').replace(/"(.*)"$/, '$1'))
    .filter((l) => l !== '');
}

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

  if (isPlaceholderSection(stat) || isPlaceholderSection(diff)) {
    return fail('429-placeholder',
      'gate diff input is a 429/rate-limit placeholder string; must fail-closed instead of vacuous GO');
  }

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
