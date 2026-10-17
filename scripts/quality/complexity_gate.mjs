// Complexity gate for the anaden-helper workspace (Issue #214, Phase 0 / T1).
//
// Runs `cccc-rs crates --ext rs` (moznion/cccc Rust front-end), excludes test
// regions (`#[cfg(test)]` items and `tests/` directories) and benches from the
// production scope, then fails (exit 1) when any production function exceeds
// the thresholds from `cccc.toml` (default: cyclomatic <= 15, cognitive <= 20).
//
// Exit codes: 0 = green, 1 = violations, 2 = environment/tool error (fail-closed).
//
// Note: the standalone `cccc-rs` 0.4.0 binary reads no config file; cccc.toml
// is the single source of thresholds for this script and enables cache only
// when the integrated `cccc` CLI is used.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const CCCC_BIN = 'cccc-rs';
const CCCC_ARGS = ['crates', '--ext', 'rs'];
const DEFAULT_MAX_CYCLOMATIC = 15;
const DEFAULT_MAX_COGNITIVE = 20;

// ---------------------------------------------------------------------------
// cccc.toml parsing (flat keys only)
// ---------------------------------------------------------------------------

/**
 * Parses the flat-key subset of cccc.toml used by this repo.
 * Invalid or missing keys fall back to defaults; `#` comments are stripped.
 */
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

// ---------------------------------------------------------------------------
// Minimal Rust lexer — yields only "code" characters
// ---------------------------------------------------------------------------

const isIdentChar = (ch) => /[A-Za-z0-9_]/.test(ch);

/**
 * Yields `{ ch, pos, line }` for characters that are NOT inside string
 * literals, char literals, or comments. Handles the traps verified against
 * this workspace: raw strings (`r"…"`, `r#"…"#`, `br…`), byte strings,
 * char-vs-lifetime disambiguation, nested block comments, and backslash +
 * newline line-continuations inside strings (the newline must still count as
 * a line break or every later line number drifts).
 * `line` is 1-based and computed from `src` start, not from `fromOffset`.
 */
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

    // Newlines advance the line counter and are not yielded as code.
    if (ch === '\n') {
      line += 1;
      i += 1;
      continue;
    }
    if (ch === '\r') {
      i += 1;
      continue;
    }

    // Line comment: skip to end of line (keep the '\n' for code state).
    if (ch === '/' && src[i + 1] === '/') {
      while (i < n && src[i] !== '\n') i += 1;
      continue;
    }

    // Block comment (nestable): `/* /* */ */`.
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

    // Raw (byte) strings: r"…", r#"…"#, br"…", br##"…"##.
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
      // Not actually a raw string (e.g. plain identifier `r#foo` macro vars are
      // not strings); fall through and emit the character as code.
    }

    // Ordinary / byte string literal: "…" and b"…".
    if (ch === 'b' && src[i + 1] === '"' && atIdentBoundary()) {
      i += 1; // consume 'b', then handle the quote below in the next iteration
      continue;
    }
    if (ch === '"') {
      i += 1;
      while (i < n && src[i] !== '"') {
        if (src[i] === '\\') {
          i += 1; // the escaped character
          if (i < n) {
            if (src[i] === '\n') {
              line += 1;
            } else if (src[i] === '\r' && src[i + 1] === '\n') {
              line += 1;
              i += 1; // consume the '\r'; the loop-end increment takes '\n'
            }
          }
          i += 1;
          continue;
        }
        if (src[i] === '\n') line += 1;
        i += 1;
      }
      i += 1; // closing quote
      continue;
    }

    // Char literals: 'x', '\n', '\'', b'x'. Everything else starting with a
    // quote is a lifetime ('a) and stays code.
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

// ---------------------------------------------------------------------------
// Test-region detection
// ---------------------------------------------------------------------------

const CFG_TEST_ATTR = /^\s*#\[\s*cfg\s*\(\s*(?:test\s*\)|all\s*\(\s*test\s*,)/;
const ATTRIBUTE_LINE = /^\s*#\[.*\]\s*$/;
const MOD_ITEM = /^\s*(?:pub(?:\([a-z ]+\))?\s+)?mod\s/;
const FN_ITEM =
  /^\s*(?:pub(?:\([a-z ]+\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s/;
const isSkippableBetweenAttrAndItem = (text) =>
  text.trim() === '' || ATTRIBUTE_LINE.test(text) || text.trimStart().startsWith('//');

/**
 * Computes 1-based `{ start, end, kind, note }` line intervals for every
 * `#[cfg(test)]` / `#[cfg(all(test, ...))]` item in a source file.
 * `start` is the item line (attribute lines are NOT included); `end` is the
 * line of the item's closing brace, or the item line itself for `;` items.
 */
export function computeTestRegions(source) {
  const lines = source.split('\n');
  const lineStarts = [0];
  for (let i = 0; i < source.length; i += 1) {
    if (source[i] === '\n') lineStarts.push(i + 1);
  }
  const regions = [];
  for (let ln = 0; ln < lines.length; ln += 1) {
    const attrLine = lines[ln];
    if (!CFG_TEST_ATTR.test(attrLine)) continue;
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
    const start = item + 1; // 1-based item line
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
    if (!closed && depth !== 0) end = lines.length; // unbalanced: exclude to EOF (fail-closed)
    regions.push({ start, end, kind, note });
  }
  return regions;
}

// ---------------------------------------------------------------------------
// Path classification & cccc output flattening
// ---------------------------------------------------------------------------

const normalizePath = (p) => String(p).replace(/\\/g, '/');

/**
 * Classifies a repo-relative path: `tests/` directory files are test code,
 * `benches/` files are out of scope for the complexity gate, everything else
 * (src/ and examples/) is scanned.
 */
export function classifyPath(relPath) {
  const p = normalizePath(relPath);
  if (/(^|\/)tests\//.test(p)) return 'tests-dir';
  if (/(^|\/)benches\//.test(p)) return 'bench';
  return 'scan';
}

const closureLabel = (fn) => (fn.kind === 'closure' ? `<closure@${fn.line}>` : fn.name);

/**
 * Flattens cccc's nested `functions`/`children` arrays (closures are reported
 * as children of their enclosing function, not at top level). `parentPath`
 * chains enclosing names, labeling closures as `<closure@line>`.
 */
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

// ---------------------------------------------------------------------------
// Gate evaluation
// ---------------------------------------------------------------------------

const severityOrder = (a, b) =>
  b.cognitive - a.cognitive || b.cyclomatic - a.cyclomatic || a.file.localeCompare(b.file) || a.line - b.line;

/**
 * Evaluates the gate against parsed cccc output.
 * `sources` maps normalized paths to file contents (scan files only are
 * consulted; a missing scan source throws — callers must pre-read and
 * fail closed instead of silently treating it as test-free production code).
 */
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
      continue; // whole file is test code — excluded from every violation list
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

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

function formatViolation(v) {
  const where = v.parentPath ? `${v.parentPath}::${v.name}` : v.name;
  return `  ${v.file}:${v.line} ${where} (${v.kind}) cyc=${v.cyclomatic} cog=${v.cognitive}`;
}

/**
 * CLI entry point. Returns the process exit code (0/1/2).
 * `opts` exists for tests: `{ root, ccccJson }` injects the repo root and the
 * parsed cccc output so no external tool is executed.
 */
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
