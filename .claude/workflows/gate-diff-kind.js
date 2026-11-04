
const DOCS_DIR_PREFIXES = ['docs/', 'doc/'];

const DOCS_ROOT_PREFIXES = ['.claude/rules/', 'CLAUDE.d/'];

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

export function classifyDiffKind(changedFiles) {
  if (!Array.isArray(changedFiles) || changedFiles.length === 0) {
    return 'code';
  }
  let sawDocs = false;
  let sawCode = false;
  for (const f of changedFiles) {
    if (typeof f !== 'string' || f.trim().length === 0) {
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

export const ALL_LANES = ([
  'reliability',
  'performance',
  'extensibility',
  'governance',
  'security',
  'integration',
]);

const CRATES_LANES = ['reliability', 'performance', 'extensibility', 'security'];

const TOOLING_LANES = ['governance', 'integration'];

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
  if (isDocsPath(p)) {
    return ['governance'];
  }
  return ALL_LANES;
}

export function sliceDiffForLanes(diffText, lanes) {
  const slices = {};
  for (const lane of lanes) {
    slices[lane] = '';
  }
  if (typeof diffText !== 'string' || diffText.length === 0) {
    return slices;
  }
  const sections = diffText.split(/^(?=diff --git )/m).filter((s) => s.startsWith('diff --git '));
  if (sections.length === 0) {
    return slices;
  }
  for (const section of sections) {
    const m = section.match(/^diff --git a\/(\S+) b\/(\S+)/);
    if (!m) {
      continue;
    }
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
