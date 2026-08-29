// Findings normalization/dedup helpers for review-gate (Issue #79).
// Canonical module. review-gate.js inlines these (the Workflow runtime rejects
// ESM imports — same pattern as review-gate-diff.js). Drift is guarded by
// review-gate-findings.test.mjs.

/** severity arbitration rank: higher wins on dedup merge */
export const SEVERITY_RANK = { critical: 4, high: 3, medium: 2, low: 1 };

/**
 * Normalize a file path for dedup keys: unify separators to '/', collapse
 * leading './' segments. Fail-open: null/undefined/'' → ''.
 */
export function normalizeFilePath(file) {
  if (typeof file !== 'string') return '';
  return file
    .replace(/\\/g, '/')
    .replace(/^(\.\/)+/, '')
    .replace(/^(\.\\)+/, '');
}

/**
 * Deterministic dedup key for a finding: normalizedFile|line|category.
 * `line` undefined/missing (e.g. maintainability schema) falls back to 0 so
 * line-less and line-0 findings of the same category on the same file collide.
 * Category is the problem-kind key (schema field, not description text).
 */
export function findingKey(finding) {
  const f = finding ?? {};
  const file = normalizeFilePath(f.file);
  const line = Number.isFinite(f.line) ? f.line : 0;
  const category = String(f.category ?? '').toLowerCase().trim();
  return `${file}|${line}|${category}`;
}

function severityOf(finding) {
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
export function mergeFindings(reviews) {
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
      if (severityOf(finding) > severityOf(m)) m.severity = finding.severity;
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
