import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const CCCC_BIN = 'cccc-rs';
const CCCC_ARGS = ['crates', '--ext', 'rs'];
const DEFAULT_MAX_CYCLOMATIC = 15;
const DEFAULT_MAX_COGNITIVE = 20;


export function parseCcccToml(text) {
  const parsed = {
    maxCyclomatic: DEFAULT_MAX_CYCLOMATIC,
    maxCognitive: DEFAULT_MAX_COGNITIVE,
    cache: false,
    cacheFile: '.cccc.cache',
  };
  for (const rawLine of String(text).split(/\r?\n/)) {
    const line = rawLine.replace(/#.*$/, '').trim();
    if (!line) continue;
    const match = /^([A-Za-z0-9_-]+)\s*=\s*(.+)$/.exec(line);
    if (!match) continue;
    const [, key, rawValue] = match;
    const value = rawValue.trim().replace(/^"(.*)"$/, '$1');
    if (key === 'max-cyclomatic' && /^\d+$/.test(value)) {
      parsed.maxCyclomatic = Number(value);
    } else if (key === 'max-cognitive' && /^\d+$/.test(value)) {
      parsed.maxCognitive = Number(value);
    } else if (key === 'cache') {
      parsed.cache = value === 'true';
    } else if (key === 'cache-file') {
      parsed.cacheFile = value;
    }
  }
  return parsed;
}


const isIdentChar = (ch) => /[A-Za-z0-9_]/.test(ch);

export function* lexRust(src, fromOffset = 0) {
  let line = 1;
  for (let i = 0; i < fromOffset; i += 1) {
    if (src[i] === '\n') line += 1;
  }
  const n = src.length;
  let i = fromOffset;
  const atIdentBoundary = () => i === 0 || !isIdentChar(src[i - 1]);
  while (i < n) {
    const ch = src[i];

    if (ch === '\n') {
      line += 1;
      i += 1;
      continue;
    }
    if (ch === '\r') {
      i += 1;
      continue;
    }

    if (ch === '/' && src[i + 1] === '/') {
      while (i < n && src[i] !== '\n') i += 1;
      continue;
    }

    if (ch === '/' && src[i + 1] === '*') {
      i += 2;
      let depth = 1;
      while (i < n && depth > 0) {
        if (src[i] === '/' && src[i + 1] === '*') {
          depth += 1;
          i += 2;
        } else if (src[i] === '*' && src[i + 1] === '/') {
          depth -= 1;
          i += 2;
        } else {
          if (src[i] === '\n') line += 1;
          i += 1;
        }
      }
      continue;
    }

    const rawPrefix =
      (ch === 'r' && (src[i + 1] === '"' || src[i + 1] === '#')) ||
      (ch === 'b' && src[i + 1] === 'r' && (src[i + 2] === '"' || src[i + 2] === '#'));
    if (rawPrefix && atIdentBoundary()) {
      let j = ch === 'b' ? i + 2 : i + 1;
      let hashes = 0;
      while (src[j] === '#') {
        hashes += 1;
        j += 1;
      }
      if (src[j] === '"') {
        i = j + 1;
        while (i < n) {
          if (src[i] === '\n') line += 1;
          if (src[i] === '"') {
            let k = i + 1;
            let seen = 0;
            while (seen < hashes && src[k] === '#') {
              seen += 1;
              k += 1;
            }
            if (seen === hashes) {
              i = k;
              break;
            }
          }
          i += 1;
        }
        continue;
      }
    }

    if (ch === 'b' && src[i + 1] === '"' && atIdentBoundary()) {
      i += 1;
      continue;
    }
    if (ch === '"') {
      i += 1;
      while (i < n && src[i] !== '"') {
        if (src[i] === '\\') {
          i += 1;
          if (i < n) {
            if (src[i] === '\n') {
              line += 1;
            } else if (src[i] === '\r' && src[i + 1] === '\n') {
              line += 1;
              i += 1;
            }
          }
          i += 1;
          continue;
        }
        if (src[i] === '\n') line += 1;
        i += 1;
      }
      i += 1;
      continue;
    }

    const byteChar = ch === 'b' && src[i + 1] === "'" && atIdentBoundary();
    const quoteIdx = byteChar ? i + 1 : ch === "'" ? i : -1;
    if (quoteIdx >= 0) {
      const next = src[quoteIdx + 1];
      const isCharLiteral =
        (next === '\\' && src[quoteIdx + 2] !== undefined && src[quoteIdx + 3] === "'") ||
        (next !== undefined && next !== '\\' && src[quoteIdx + 2] === "'");
      if (isCharLiteral) {
        i = quoteIdx + (next === '\\' ? 4 : 3);
        continue;
      }
    }

    yield { ch, pos: i, line };
    i += 1;
  }
}


const stripStringValues = (s) => s.replace(/"[^"]*"/g, '""');
const cfgContainsTest = (line) => {
  const header = /^\s*#\[\s*cfg\s*\(\s*/.exec(line);
  if (!header) return false;
  const rest = line.slice(header[0].length);
  if (/^test\s*\)/.test(rest)) return true;
  const combo = /^(?:all|any)\s*\(/.exec(rest);
  if (!combo) return false;
  let depth = 1;
  let i = combo[0].length;
  while (i < rest.length && depth > 0) {
    if (rest[i] === '(') depth += 1;
    else if (rest[i] === ')') depth -= 1;
    i += 1;
  }
  const inner = rest.slice(combo[0].length, i - 1);
  return /(?:^|[,\s(])test(?:[,\s)]|$)/.test(stripStringValues(inner));
};
const ATTRIBUTE_LINE = /^\s*#\[.*\]\s*$/;
const MOD_ITEM = /^\s*(?:pub(?:\([a-z ]+\))?\s+)?mod\s/;
const FN_ITEM =
  /^\s*(?:pub(?:\([a-z ]+\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s/;
const isSkippableBetweenAttrAndItem = (text) =>
  text.trim() === '' || ATTRIBUTE_LINE.test(text) || text.trimStart().startsWith('//');

export function computeTestRegions(source) {
  const lines = source.split('\n');
  const lineStarts = [0];
  for (let i = 0; i < source.length; i += 1) {
    if (source[i] === '\n') lineStarts.push(i + 1);
  }
  const regions = [];
  for (let ln = 0; ln < lines.length; ln += 1) {
    const attrLine = lines[ln];
    if (!cfgContainsTest(attrLine)) continue;
    const note = /all\s*\(/.test(attrLine) ? 'cfg(all(test,...))' : 'cfg(test)';
    let item = ln + 1;
    while (item < lines.length && isSkippableBetweenAttrAndItem(lines[item])) {
      item += 1;
    }
    if (item >= lines.length) continue;
    const itemText = lines[item];
    let kind = 'other';
    if (MOD_ITEM.test(itemText)) kind = 'mod';
    else if (FN_ITEM.test(itemText)) kind = 'fn';
    const attrStart = ln + 1;
    const start = item + 1;
    let end = lines.length;
    let depth = 0;
    let closed = false;
    for (const { ch, line } of lexRust(source, lineStarts[item] ?? source.length)) {
      if (depth === 0 && ch === ';') {
        end = start;
        closed = true;
        break;
      }
      if (ch === '{') {
        depth += 1;
      } else if (ch === '}') {
        depth -= 1;
        if (depth === 0) {
          end = line;
          closed = true;
          break;
        }
      }
    }
    if (!closed && depth !== 0) end = lines.length;
    regions.push({ start, end, kind, note, attrStart });
  }
  return regions;
}


const normalizePath = (p) => String(p).replace(/\\/g, '/');

export function classifyPath(relPath) {
  const p = normalizePath(relPath);
  if (/(^|\/)tests\//.test(p)) return 'tests-dir';
  if (/(^|\/)benches\//.test(p)) return 'bench';
  return 'scan';
}

const closureLabel = (fn) => (fn.kind === 'closure' ? `<closure@${fn.line}>` : fn.name);

export function flattenFunctions(ccccFiles) {
  const out = [];
  const walk = (file, fns, parents) => {
    for (const fn of fns) {
      out.push({
        file,
        name: fn.name,
        kind: fn.kind,
        line: fn.line,
        cognitive: fn.cognitive,
        cyclomatic: fn.cyclomatic,
        parentPath: parents.join('::'),
      });
      if (Array.isArray(fn.children) && fn.children.length > 0) {
        walk(file, fn.children, [...parents, closureLabel(fn)]);
      }
    }
  };
  for (const fileEntry of ccccFiles) {
    walk(normalizePath(fileEntry.path), fileEntry.functions ?? [], []);
  }
  return out;
}


const severityOrder = (a, b) =>
  b.cognitive - a.cognitive || b.cyclomatic - a.cyclomatic || a.file.localeCompare(b.file) || a.line - b.line;

export function evaluateGate({ ccccJson, sources, thresholds }) {
  const maxCyclomatic = thresholds?.maxCyclomatic ?? DEFAULT_MAX_CYCLOMATIC;
  const maxCognitive = thresholds?.maxCognitive ?? DEFAULT_MAX_COGNITIVE;
  const isOver = (fn) => fn.cyclomatic > maxCyclomatic || fn.cognitive > maxCognitive;
  const toRecord = (fn) => ({
    file: fn.file,
    line: fn.line,
    name: fn.name,
    kind: fn.kind,
    cyclomatic: fn.cyclomatic,
    cognitive: fn.cognitive,
    parentPath: fn.parentPath,
  });

  const flat = flattenFunctions(ccccJson.files ?? []);
  const byFile = new Map();
  for (const fn of flat) {
    if (!byFile.has(fn.file)) byFile.set(fn.file, []);
    byFile.get(fn.file).push(fn);
  }

  const counts = {
    files: ccccJson.files?.length ?? 0,
    functionsTotal: flat.length,
    production: 0,
    testExcluded: 0,
    benchOutOfScope: 0,
    violations: 0,
  };
  const violations = [];
  const excludedTestViolations = [];
  const benchViolations = [];
  const testRegions = { files: [], regions: [] };

  for (const [file, fns] of byFile) {
    const classification = classifyPath(file);
    if (classification === 'tests-dir') {
      testRegions.files.push({ file, basis: 'tests-dir' });
      counts.testExcluded += fns.length;
      continue;
    }
    if (classification === 'bench') {
      counts.benchOutOfScope += fns.length;
      for (const fn of fns) {
        if (isOver(fn)) benchViolations.push(toRecord(fn));
      }
      continue;
    }
    const source = sources[file];
    if (typeof source !== 'string') {
      throw new Error(`missing source for scanned file: ${file}`);
    }
    const regions = computeTestRegions(source);
    for (const region of regions) {
      testRegions.regions.push({ file, start: region.start, end: region.end, kind: region.kind });
    }
    for (const fn of fns) {
      const inTestRegion = regions.some((r) => fn.line >= r.start && fn.line <= r.end);
      if (inTestRegion) {
        counts.testExcluded += 1;
        if (isOver(fn)) excludedTestViolations.push(toRecord(fn));
      } else {
        counts.production += 1;
        if (isOver(fn)) violations.push(toRecord(fn));
      }
    }
  }

  violations.sort(severityOrder);
  excludedTestViolations.sort(severityOrder);
  benchViolations.sort(severityOrder);
  counts.violations = violations.length;
  return { counts, violations, excludedTestViolations, benchViolations, testRegions };
}


const scriptDir = path.dirname(fileURLToPath(import.meta.url));

function formatViolation(v) {
  const where = v.parentPath ? `${v.parentPath}::${v.name}` : v.name;
  return `  ${v.file}:${v.line} ${where} (${v.kind}) cyc=${v.cyclomatic} cog=${v.cognitive}`;
}

export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');

  let ccccJson = opts.ccccJson;
  let toolVersion = 'injected';
  if (ccccJson === undefined) {
    const versionRun = spawnSync(CCCC_BIN, ['--version'], { encoding: 'utf8' });
    if (versionRun.error || versionRun.status !== 0) {
      console.error(`complexity gate: '${CCCC_BIN}' not available (install cargo-cccc / cccc-rs)`);
      return 2;
    }
    toolVersion = versionRun.stdout.trim();
    const run = spawnSync(CCCC_BIN, CCCC_ARGS, {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    });
    if (run.error || run.status !== 0 || !run.stdout) {
      console.error(`complexity gate: '${CCCC_BIN} ${CCCC_ARGS.join(' ')}' failed (status=${run.status})`);
      return 2;
    }
    try {
      ccccJson = JSON.parse(run.stdout);
    } catch (error) {
      console.error(`complexity gate: cannot parse ${CCCC_BIN} JSON output: ${error.message}`);
      return 2;
    }
  }

  let thresholds = { maxCyclomatic: DEFAULT_MAX_CYCLOMATIC, maxCognitive: DEFAULT_MAX_COGNITIVE };
  let thresholdSource = 'defaults';
  const tomlPath = path.join(root, 'cccc.toml');
  if (fs.existsSync(tomlPath)) {
    try {
      thresholds = parseCcccToml(fs.readFileSync(tomlPath, 'utf8'));
      thresholdSource = 'cccc.toml';
    } catch {
      thresholds = { maxCyclomatic: DEFAULT_MAX_CYCLOMATIC, maxCognitive: DEFAULT_MAX_COGNITIVE };
    }
  }

  const sources = {};
  for (const fileEntry of ccccJson.files ?? []) {
    const rel = normalizePath(fileEntry.path);
    if (classifyPath(rel) !== 'scan') continue;
    try {
      sources[rel] = fs.readFileSync(path.join(root, ...rel.split('/')), 'utf8');
    } catch (error) {
      console.error(`complexity gate: cannot read scanned file ${rel}: ${error.message}`);
      return 2;
    }
  }

  let result;
  try {
    result = evaluateGate({ ccccJson, sources, thresholds });
  } catch (error) {
    console.error(`complexity gate: evaluation failed: ${error.message}`);
    return 2;
  }

  const report = {
    generatedAt: new Date().toISOString(),
    tool: { name: CCCC_BIN, version: toolVersion, args: [...CCCC_ARGS] },
    thresholds: {
      maxCyclomatic: thresholds.maxCyclomatic,
      maxCognitive: thresholds.maxCognitive,
      source: thresholdSource,
    },
    counts: result.counts,
    violations: result.violations,
    excludedTestViolations: result.excludedTestViolations,
    benchViolations: result.benchViolations,
    testRegions: result.testRegions,
    summary: ccccJson.summary ?? null,
  };
  const reportPath = path.join(root, 'target', 'quality', 'cccc-report.json');
  try {
    fs.mkdirSync(path.dirname(reportPath), { recursive: true });
    fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  } catch (error) {
    console.error(`complexity gate: cannot write report ${reportPath}: ${error.message}`);
    return 2;
  }

  const { counts } = result;
  console.log(
    `complexity gate: thresholds cyc<=${thresholds.maxCyclomatic} cog<=${thresholds.maxCognitive} (${thresholdSource})`,
  );
  console.log(
    `  functions: ${counts.functionsTotal} total = ${counts.production} production + ` +
      `${counts.testExcluded} test-excluded + ${counts.benchOutOfScope} bench (out of scope)`,
  );
  console.log(`  test regions: ${result.testRegions.regions.length} in scanned files; ` +
    `${result.testRegions.files.length} tests/ files excluded wholesale`);
  if (result.excludedTestViolations.length > 0) {
    console.log(`  over-threshold inside test regions (excluded): ${result.excludedTestViolations.length}`);
    for (const v of result.excludedTestViolations) console.log(formatViolation(v));
  }
  if (counts.violations > 0) {
    console.log(`violations: ${counts.violations}`);
    for (const v of result.violations) console.log(formatViolation(v));
    return 1;
  }
  console.log('violations: 0');
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
