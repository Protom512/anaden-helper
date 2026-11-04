
export function extractChangedPaths(diff) {
  const paths = new Set();
  if (typeof diff !== 'string') return [...paths];
  const gitLines = diff.matchAll(/^diff --git a\/(\S+) b\/(\S+)$/gm);
  for (const m of gitLines) {
    paths.add(m[1]);
    paths.add(m[2]);
  }
  const plusLines = diff.matchAll(/^\+\+\+ b\/(\S+)$/gm);
  for (const m of plusLines) paths.add(m[1]);
  const statLines = diff.matchAll(/^\s+(\S+?)\s+\|\s+\d+/gm);
  for (const m of statLines) {
    if (!m[1].startsWith('...') && m[1] !== '') paths.add(m[1]);
  }
  return [...paths];
}

function titleKeywords(title) {
  if (typeof title !== 'string') return [];
  return title
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((w) => w.length >= 3);
}

function pathMatchesKeyword(path, kw) {
  const p = path.toLowerCase();
  return p.includes(kw) || path.split('/').pop().toLowerCase().includes(kw);
}

export function testContrast({ ticketTitle, designFiles = [], taskFiles = [], diff }) {
  const mismatches = [];
  const changed = extractChangedPaths(diff);
  const kws = titleKeywords(ticketTitle);
  const declared = [...designFiles, ...taskFiles].filter((f) => typeof f === 'string');

  const overlapsTitle =
    changed.length > 0 &&
    (changed.some((p) => kws.some((kw) => pathMatchesKeyword(p, kw))) ||
      changed.some((p) => declared.includes(p)));
  if (!overlapsTitle) {
    mismatches.push({
      kind: 'title-diff',
      detail:
        changed.length === 0
          ? 'diff is empty — no changed files to contrast against ticket intent (fail-closed signal)'
          : `changed files [${changed.join(', ')}] do not overlap ticket title keywords [${kws.join(', ')}]`,
    });
  }

  if (declared.length > 0 && changed.length > 0) {
    const declaredBasenames = declared.map((f) => f.split('/').pop().toLowerCase());
    const overlapsDeclared = changed.some(
      (p) =>
        declared.includes(p) ||
        declaredBasenames.includes(p.split('/').pop().toLowerCase())
    );
    if (!overlapsDeclared) {
      mismatches.push({
        kind: 'design-tasks',
        detail: `changed files [${changed.join(', ')}] do not overlap design/tasks declared files [${declared.join(', ')}]`,
      });
    }
  }

  return { consistent: mismatches.length === 0, mismatches };
}
