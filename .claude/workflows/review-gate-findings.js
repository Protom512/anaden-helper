
export const SEVERITY_RANK = { critical: 4, high: 3, medium: 2, low: 1 };

export function normalizeFilePath(file) {
  if (typeof file !== 'string') return '';
  return file
    .replace(/\\/g, '/')
    .replace(/^(\.\/)+/, '')
    .replace(/^(\.\\)+/, '');
}

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
