// R8 diff-inject (Issue #78): helpers ported from feature-pipeline.js
// (L589-635) so reviewer prompts carry the actual working-tree diff instead of
// only a changed-file name list. Cap values kept consistent with feature-pipeline
// (28000/24000/30000). NOTE: inlined (not imported) because the Workflow runtime
// rejects ESM imports — the script must be self-contained for scriptPath launch.
// review-gate-diff.js remains the canonical helper module; keep values in sync
// (guarded by tests/review-gate-diff-inject.test.mjs drift check).

// ── inlined from review-gate-findings.js (canonical copy: .claude/workflows/review-gate-findings.js) ──
// Issue #79: findings dedup/normalization before the Judge (QC Manager) phase.
// Drift guarded by review-gate-findings.test.mjs.

/** severity arbitration rank: higher wins on dedup merge */
const SEVERITY_RANK = { critical: 4, high: 3, medium: 2, low: 1 };

/** Normalize a file path for dedup keys (unify separators, strip leading ./). Fail-open. */
function normalizeFilePath(file) {
  if (typeof file !== 'string') return '';
  return file
    .replace(/\\/g, '/')
    .replace(/^(\.\/)+/, '')
    .replace(/^(\.\\)+/, '');
}

/**
 * Deterministic dedup key for a finding: normalizedFile|line|category.
 * `line` undefined/missing (e.g. line-less maintainability findings) falls back
 * to 0 so line-less and line-0 findings of the same category on the same file
 * collide. Category is the problem-kind key (schema field, not description text).
 */
function findingKey(finding) {
  const f = finding ?? {};
  const file = normalizeFilePath(f.file);
  const line = Number.isFinite(f.line) ? f.line : 0;
  const category = String(f.category ?? '').toLowerCase().trim();
  return `${file}|${line}|${category}`;
}

function severityRankOf(finding) {
  return SEVERITY_RANK[finding.severity] ?? 0;
}

/**
 * Merge multiple reviewers' findings into a deduped list keyed by findingKey.
 * - severity: max across reviewers (critical > high > medium > low)
 * - reviewers: all reviewer labels that reported the key
 * - description: all distinct descriptions joined with ' / '
 * - fix: first non-empty fix string
 * Reviews without findings / null input → [] (fail-open).
 */
function mergeFindings(reviews) {
  const merged = new Map();
  for (const review of reviews ?? []) {
    for (const finding of review?.findings ?? []) {
      const key = findingKey(finding);
      if (!merged.has(key)) {
        merged.set(key, {
          key,
          file: normalizeFilePath(finding.file),
          line: Number.isFinite(finding.line) ? finding.line : 0,
          category: String(finding.category ?? '').toLowerCase().trim(),
          severity: finding.severity,
          description: finding.description ?? '',
          fix: finding.fix ?? '',
          reviewers: [review.reviewer ?? 'unknown'],
        });
        continue;
      }
      const m = merged.get(key);
      if (severityRankOf(finding) > severityRankOf(m)) m.severity = finding.severity;
      if (!m.reviewers.includes(review.reviewer ?? 'unknown')) {
        m.reviewers.push(review.reviewer ?? 'unknown');
      }
      const desc = finding.description ?? '';
      if (desc && !m.description.includes(desc)) {
        m.description = m.description ? `${m.description} / ${desc}` : desc;
      }
      if (!m.fix && finding.fix) m.fix = finding.fix;
    }
  }
  return [...merged.values()];
}

// ── inlined from review-gate-diff.js (canonical copy: .claude/workflows/review-gate-diff.js) ──
const DIFF_TRUNCATE_THRESHOLD = 28000;
const DIFF_BODY_CAP = 24000;
const TOTAL_DIFF_CAP = 30000;

const DIFF_FETCH_PROMPT = `Diff fetcher for the Review Gate (Issue #102 T4 — feature-pipeline gate:fetch-diff と同一の収集内容). In the repo CWD run these and return ONE plain-text string (no JSON, no commentary wrapper):
1. \`git --no-pager diff HEAD --stat\`
2. \`git --no-pager diff HEAD\`
3. \`git --no-pager status --porcelain\`
4. \`git --no-pager diff HEAD~1..HEAD --stat\` (commit-range)
5. \`git --no-pager diff HEAD~1..HEAD\`
6. \`git --no-pager diff origin/master...HEAD --stat\` (merge-base commit-range — HEAD~1 が解決不能な merge context や複数 commit の slice 用)
7. \`git --no-pager diff origin/master...HEAD\` (merge-base full diff)
8. \`git write-tree\` (tree hash — 「green だが実体は空」検出用)
Concatenate with headers "=== STAT ===", "=== DIFF ===", "=== UNTRACKED ===", "=== COMMIT-RANGE STAT ===", "=== COMMIT-RANGE DIFF ===", "=== MERGE-BASE STAT ===", "=== MERGE-BASE DIFF ===", "=== TREE HASH ===". If the DIFF, COMMIT-RANGE DIFF or MERGE-BASE DIFF section exceeds ${DIFF_TRUNCATE_THRESHOLD} chars, emit full STAT + UNTRACKED + the other sections but only the FIRST ${DIFF_BODY_CAP} chars of the oversized DIFF section, then a line "[DIFF TRUNCATED]". Return ONLY the concatenated text.`;

// ── inlined from review-gate-diff.js (Issue #91 T1 canonical copy, T3 inline) ──
// buildCommitRangeDiffInput: working-tree diff が空 (commit 済み slice) の場合は
// commit-range diff (HEAD~1..HEAD / merge-base) へ fallback し、tree hash を
// evidence に添付する。両方空なら fail-closed — 空の diff を reviewer に注入して
// vacuous GO させることはしない (pipeline-evidence-verification.md §2)。
// Drift guarded by tests/review-gate-diff-inject.test.mjs (input/output pair comparison).

/** Commit-range spec per variant. */
const RANGE_VARIANTS = {
  'head-prev': 'HEAD~1..HEAD',
  'merge-base': 'origin/master...HEAD',
};

function buildCommitRangeDiffInput(input, options = {}) {
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

// ── inlined from review-gate-diff.js (Issue #102 T1 canonical copy, T4 inline) ──
// buildUnifiedGateDiff: 決定論的 fallback チェーン (単一情報源)。
//   (a) working-tree STAT/DIFF 非空 → 'working-tree'
//   (b) 空 → HEAD~1..HEAD commit-range
//   (c) まだ空 → merge-base (origin/master...HEAD)
//   (d) untracked のみ → 'intent-to-add' (file list + 個別 Read)
//   (e) 全空 / 429 placeholder → 'fail-closed'
// 429 placeholder 検出は exact match (trimmed section === sentinel) に限定 —
// 正当な diff 内の部分一致で fail-closed にしない (estimate approval condition #1)。
// Drift guarded by tests/review-gate-unified-diff.test.mjs (input/output pair comparison).

/** Exact-match 429/placeholder sentinels. */
const PLACEHOLDER_SENTINELS = ['429', 'rate limit', 'placeholder'];

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

/** Parse `git status --porcelain` untracked lines into file paths. */
function parseUntrackedFiles(untracked) {
  return untracked
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.startsWith('??'))
    .map((l) => l.replace(/^\?\?\s+/, '').replace(/"(.*)"$/, '$1'))
    .filter((l) => l !== '');
}

function buildUnifiedGateDiff(input) {
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

function extractDiffSection(raw, name) {
  if (typeof raw !== 'string' || raw === '') return '';
  const startMarker = `=== ${name} ===`;
  const start = raw.indexOf(startMarker);
  if (start < 0) return '';
  const bodyStart = start + startMarker.length;
  const next = raw.indexOf('===', bodyStart);
  const body = next < 0 ? raw.slice(bodyStart) : raw.slice(bodyStart, next);
  return body.trim();
}

function normalizeGateDiff(result) {
  const text = typeof result === 'string'
    ? result
    : (result == null ? '' : JSON.stringify(result));
  return text.slice(0, TOTAL_DIFF_CAP);
}

function buildDiffSection(diff) {
  if (!diff) return '';
  return `=== WORKING-TREE DIFF ===
${diff}
=== END DIFF ===`;
}

function withDiffContext(prompt, diffSection) {
  if (!diffSection) return prompt;
  return `${prompt}

【提供済み diff で直接分析（R8 — diff-inject, Issue #78）】下記に working-tree の diff を示す。
これを直接読んで分析すること。ファイルを個別に Read して再取得*しない*こと（diff が手元にある）。
あなたの役割に特有の grep/check は1回まで許可するが、tool 使用は最小限にし、最終アクションは
*必ず* StructuredOutput を呼ぶこと（prose-only で終わらせない）。findings の file/line は
この diff の hunk に基づくこと（working tree の現状態ではなく）。

${diffSection}`;
}

export const meta = {
  name: 'review-gate',
  description: 'Multi-reviewer QC gate: Architecture + Functional + Maintainability parallel review with consensus judgment',
  phases: [
    { title: 'Analyze', detail: 'Identify changed files and review scope' },
    { title: 'Review', detail: 'Parallel 3-reviewer assessment' },
    { title: 'Judge', detail: 'QC Manager synthesizes and decides GO/NO-GO' },
  ],
};

// Phase 1: Analyze changes
phase('Analyze');
const changes = await agent(
  `Analyze the current git changes for review scope.

Run:
1. git diff --name-only HEAD (or git diff --name-only --staged)
2. For each changed file, categorize it:
   - Which crate does it belong to?
   - Is it library code or test code?
   - What type of change (new, modified, deleted)?

3. Determine review scope:
   - Standard: 3 reviewers (all library changes)
   - Light: 1-2 reviewers (docs only, test only)
   - Security: 4 reviewers (auth, crypto, input handling)

Return the analysis.`,
  { label: 'qc:analyze', phase: 'Analyze', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      changedFiles: { type: 'array', items: { type: 'string' } },
      affectedCrates: { type: 'array', items: { type: 'string' } },
      changeType: { type: 'string', enum: ['standard', 'light', 'security'] },
      hasLibraryChanges: { type: 'boolean' },
      hasTestChanges: { type: 'boolean' },
    },
    required: ['changedFiles', 'affectedCrates', 'changeType'],
  }}
);
log(`Review scope: ${changes.changeType} (${changes.changedFiles.length} files)`);

// ── Issue #78: R8 diff-inject (ported from feature-pipeline.js) ──
// reviewer が changedFiles 名一覧のみでレビューすると指摘が PR diff 由来である
// 保証がない。working-tree diff を1回取得して全 reviewer プロンプトへ埋め込み、
// 「探索」を「提供済み diff の直接分析」へ変える (R8 パターンの移植)。
const diffFetch = await agent(
  DIFF_FETCH_PROMPT,
  { label: 'review:fetch-diff', phase: 'Analyze' }
);
const GATE_DIFF_RAW = (typeof diffFetch === 'string'
  ? diffFetch
  : (diffFetch == null ? '' : JSON.stringify(diffFetch))
).slice(0, TOTAL_DIFF_CAP);
// Issue #102 T4: working-tree → commit-range (HEAD~1..HEAD) → merge-base
// (origin/master...HEAD) → untracked-only (intent-to-add) → fail-closed の
// 決定論的 fallback チェーン (T1 buildUnifiedGateDiff inline copy)。
// 空 diff・429 placeholder は reviewer に注入せず fail-closed — vacuous GO
// させない (feature-pipeline.js gate と同一挙動のミラー適用,
// pipeline-evidence-verification.md §2)。
const gateDiff = buildUnifiedGateDiff({
  stat: extractDiffSection(GATE_DIFF_RAW, 'STAT'),
  diff: extractDiffSection(GATE_DIFF_RAW, 'DIFF'),
  untracked: extractDiffSection(GATE_DIFF_RAW, 'UNTRACKED'),
  headPrevStat: extractDiffSection(GATE_DIFF_RAW, 'COMMIT-RANGE STAT'),
  headPrevDiff: extractDiffSection(GATE_DIFF_RAW, 'COMMIT-RANGE DIFF'),
  mergeBaseStat: extractDiffSection(GATE_DIFF_RAW, 'MERGE-BASE STAT'),
  mergeBaseDiff: extractDiffSection(GATE_DIFF_RAW, 'MERGE-BASE DIFF'),
  treeHash: extractDiffSection(GATE_DIFF_RAW, 'TREE HASH') || null,
});
if (gateDiff.mode === 'fail-closed') {
  // fail-closed: 空 diff / 429 placeholder では reviewer に注入しない。
  // NO-GO として即時終了。
  log(`Review Gate DIFF-EMPTY-FAILED: ${gateDiff.reason}${gateDiff.note ? ` note=${gateDiff.note}` : ''} — gate を短絡する (Issue #102 T4, fail-closed, basis=${gateDiff.basis})`);
  return {
    status: 'DIFF_EMPTY_FAILED',
    diffInput: gateDiff,
    judgment: {
      decision: 'NO-GO',
      summary: `diff が空または 429 placeholder (${gateDiff.reason})。commit 済み slice の場合は commit-range diff (HEAD~1..HEAD → merge-base) が自動使用されるが、それも空だった。対象コミットが存在するかスライス範囲を確認して再実行のこと。`,
      requiredFixes: [{
        priority: 'critical',
        reviewer: 'review-gate',
        description: `diff empty: ${gateDiff.reason}`,
        fix: 'コミット済み slice の commit-range diff (HEAD~1..HEAD / merge-base) が空。スライス範囲を確認し pipeline 再実行のこと。',
      }],
    },
  };
}
// intent-to-add mode: tracked diff は無いが untracked ファイルがある。
// snapshot (untracked file list) を注入し、reviewer に個別 Read を促す
// (vacuous GO 防止 — S2 修正案 UC-1 (d))。
const REVIEW_DIFF_RAW = gateDiff.mode === 'intent-to-add'
  ? `=== UNTRACKED FILES (intent-to-add; Read each file individually) ===\n${gateDiff.untrackedFiles.join('\n')}`.slice(0, TOTAL_DIFF_CAP)
  : [
    '=== STAT ===',
    gateDiff.stat,
    '=== DIFF ===',
    gateDiff.diff,
    '=== UNTRACKED ===',
    gateDiff.untracked,
    ...(gateDiff.treeHash ? ['=== TREE HASH ===', gateDiff.treeHash] : []),
  ].join('\n').slice(0, TOTAL_DIFF_CAP);
const REVIEW_DIFF_SECTION = buildDiffSection(REVIEW_DIFF_RAW);
log(`Review Gate: injected diff context (${REVIEW_DIFF_SECTION.length} chars, mode=${gateDiff.mode}, basis=${gateDiff.basis}${gateDiff.rangeVariant ? ` variant=${gateDiff.rangeVariant}` : ''}${gateDiff.mode === 'intent-to-add' ? ` files=${gateDiff.untrackedFiles.length}` : ''}) into all reviewers (Issue #78 R8 / Issue #102 T4)`);

// Phase 2: Parallel reviews
phase('Review');
const reviewers = [];

// Always include Architecture reviewer for library changes
if (changes.hasLibraryChanges || changes.changeType === 'standard') {
  reviewers.push(() => agent(
    withDiffContext(`Architecture Review (MAGI MELCHIOR):

Changed files: ${changes.changedFiles.join(', ')}

Run these MANDATORY checks:
\`\`\`bash
cargo check --all --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
git --no-pager diff HEAD
\`\`\`

Cross-check (Issue #78): every finding's file/line MUST match a hunk in the
\`git --no-pager diff HEAD\` output injected below. Findings that do not
correspond to a changed line in the diff are out of scope — do not report them.

For each changed file, verify:
1. Module boundaries respected
2. No circular dependencies introduced
3. Types are used correctly
4. No anti-patterns (unwrap/expect/panic in library code)
5. Code is idiomatic Rust

Decision: GO or NO-GO`, REVIEW_DIFF_SECTION),
    { label: 'review:architecture', phase: 'Review', model: 'sonnet', schema: {
      type: 'object',
      properties: {
        decision: { type: 'string', enum: ['GO', 'NO-GO'] },
        checks: { type: 'object', properties: {
          compilation: { type: 'string', enum: ['PASS', 'FAIL'] },
          clippy: { type: 'string', enum: ['PASS', 'FAIL'] },
          fmt: { type: 'string', enum: ['PASS', 'FAIL'] },
        }},
        findings: { type: 'array', items: { type: 'object', properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          description: { type: 'string' },
          file: { type: 'string' },
          line: { type: 'number' },
          category: { type: 'string' },
          fix: { type: 'string' },
        }}},
      },
      required: ['decision', 'checks', 'findings'],
    }}
  ));
}

// Always include Functional reviewer for any changes
reviewers.push(() => agent(
  withDiffContext(`Functional Review (MAGI BALTHASAR):

Changed files: ${changes.changedFiles.join(', ')}

Run this MANDATORY check:
\`\`\`bash
cargo nextest run --workspace
git --no-pager diff HEAD
\`\`\`

Cross-check (Issue #78): every finding's file/line MUST match a hunk in the
\`git --no-pager diff HEAD\` output injected below. Findings that do not
correspond to a changed line in the diff are out of scope — do not report them.

Verify:
1. All tests pass
2. New tests cover the changes
3. Edge cases are tested
4. Error paths are exercised
5. No ignored tests without justification

Decision: GO or NO-GO`, REVIEW_DIFF_SECTION),
  { label: 'review:functional', phase: 'Review', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      decision: { type: 'string', enum: ['GO', 'NO-GO'] },
      testsPassed: { type: 'boolean' },
      totalTests: { type: 'number' },
      findings: { type: 'array', items: { type: 'object', properties: {
        severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
        description: { type: 'string' },
        file: { type: 'string' },
        line: { type: 'number' },
        category: { type: 'string' },
        fix: { type: 'string' },
      }}},
    },
    required: ['decision', 'testsPassed', 'findings'],
  }}
));

// Include Maintainability reviewer for library changes
if (changes.hasLibraryChanges || changes.changeType === 'standard') {
  reviewers.push(() => agent(
    withDiffContext(`Maintainability Review (MAGI CASPER):

Changed files: ${changes.changedFiles.join(', ')}

Run this MANDATORY check:
\`\`\`bash
git --no-pager diff HEAD
\`\`\`

Cross-check (Issue #78): every finding's file/line MUST match a hunk in the
\`git --no-pager diff HEAD\` output injected below. Findings that do not
correspond to a changed line in the diff are out of scope — do not report them.

Check:
1. grep -r "unwrap()\\|expect(\\|panic!" in changed library files
2. Public API documentation exists (/// comments)
3. Code follows Rust style (.claude/rules/rust-style.md)
4. No anti-patterns (.claude/rules/rust-anti-patterns.md)
5. Test modules have #[allow] annotations
6. Function length is reasonable (<50 lines typical)

Decision: GO or NO-GO`, REVIEW_DIFF_SECTION),
    { label: 'review:maintainability', phase: 'Review', model: 'sonnet', schema: {
      type: 'object',
      properties: {
        decision: { type: 'string', enum: ['GO', 'NO-GO'] },
        antiPatternsFound: { type: 'number' },
        undocumentedApis: { type: 'number' },
        findings: { type: 'array', items: { type: 'object', properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          description: { type: 'string' },
          file: { type: 'string' },
          line: { type: 'number' },
          category: { type: 'string' },
          fix: { type: 'string' },
        }}},
      },
      required: ['decision', 'findings'],
    }}
  ));
}

const reviews = await parallel(reviewers);
const validReviews = reviews.filter(Boolean);

// Issue #79: normalize + dedup findings across reviewers (severity max arbitration)
const mergedFindings = mergeFindings(validReviews);
log(`Findings after dedup: ${mergedFindings.length} (Issue #79)`);

log(`Reviews completed: ${validReviews.length} reviewers`);

// Phase 3: QC Manager judges
phase('Judge');
const judgment = await agent(
  `You are the QC Manager. Synthesize these review results and make a final GO/NO-GO judgment.

Review Results:
${JSON.stringify(validReviews, null, 2)}

Deduped/normalized findings (file+line+category key, severity = max across reviewers):
${JSON.stringify(mergedFindings, null, 2)}

Consensus Rules:
- ALL reviewers must say GO for overall GO
- ANY single NO-GO means overall NO-GO
- Critical findings must be fixed regardless

If NO-GO, list ALL required fixes organized by priority:
1. Critical (must fix)
2. High (strongly recommended)
3. Medium (consider)
4. Low (optional)

Provide the final QC Report.`,
  { label: 'qc-manager:judge', phase: 'Judge', model: 'opus', schema: {
    type: 'object',
    properties: {
      decision: { type: 'string', enum: ['GO', 'NO-GO'] },
      summary: { type: 'string' },
      requiredFixes: { type: 'array', items: { type: 'object', properties: {
        priority: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
        reviewer: { type: 'string' },
        description: { type: 'string' },
        file: { type: 'string' },
        fix: { type: 'string' },
      }}},
      recommendations: { type: 'array', items: { type: 'string' } },
    },
    required: ['decision', 'summary', 'requiredFixes'],
  }}
);

log(`QC Judgment: ${judgment.decision} - ${judgment.summary}`);

return {
  scope: changes,
  reviews: validReviews,
  judgment,
};
