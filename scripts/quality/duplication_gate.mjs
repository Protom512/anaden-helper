// Duplication gate for the anaden-helper workspace (Issue #214, Phase 0 / T2).
//
// Runs `similarity-rs crates -t 0.80 -m 15 --skip-test`, keeps only pairs where
// BOTH functions live in production scope (test regions, `tests/` dirs and
// `benches/` are machine-excluded — similarity's --skip-test only skips
// #[test] functions, not helpers inside `#[cfg(test)] mod tests`), then fails
// (exit 1) when a non-allowlisted production pair is >= 0.85 similar.
// Pairs in 0.80-0.84 are advisory only.
//
// Exit codes: 0 = green, 1 = violations, 2 = environment/tool error (fail-closed).
//
// Test-region semantics are shared with complexity_gate.mjs (single lexer /
// region algorithm — import, do not duplicate).
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { computeTestRegions, classifyPath } from './complexity_gate.mjs';

const SIMILARITY_BIN = 'similarity-rs';
const SIMILARITY_ARGS = ['crates', '-t', '0.80', '-m', '15', '--skip-test'];
const VIOLATION_THRESHOLD = 0.85;

// ---------------------------------------------------------------------------
// similarity-rs output parsing
// ---------------------------------------------------------------------------

const PAIR_LINE =
  /^  (\S+?):(\d+)-(\d+) (\w+) (\S+) <-> (\S+?):(\d+)-(\d+) (\w+) (\S+)$/;
const SIMILARITY_LINE = /^  Similarity: ([\d.]+)%$/;

const normalizePath = (p) => String(p).replace(/\\/g, '/');

/**
 * Parses similarity-rs text output into pairs:
 * `{ a: {file,start,end,kind,name}, b: {...}, similarity }` where `similarity`
 * is 0..1 (percent / 100). Header lines, separators and `Classes:` context
 * lines are ignored. Paths are normalized to `/` separators.
 */
export function parseSimilarityOutput(text) {
  const pairs = [];
  let pending = null;
  for (const raw of String(text).split(/\r?\n/)) {
    const pairMatch = PAIR_LINE.exec(raw);
    if (pairMatch) {
      pending = {
        a: {
          file: normalizePath(pairMatch[1]),
          start: Number(pairMatch[2]),
          end: Number(pairMatch[3]),
          kind: pairMatch[4],
          name: pairMatch[5],
        },
        b: {
          file: normalizePath(pairMatch[6]),
          start: Number(pairMatch[7]),
          end: Number(pairMatch[8]),
          kind: pairMatch[9],
          name: pairMatch[10],
        },
      };
      continue;
    }
    const simMatch = SIMILARITY_LINE.exec(raw);
    if (simMatch && pending) {
      pending.similarity = Number(simMatch[1]) / 100;
      pairs.push(pending);
      pending = null;
    }
  }
  return pairs;
}

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

/**
 * Returns the matching allowlist entry when the pair's file matches and both
 * function names appear in the entry's `functions` list, else null.
 * Matching is order-insensitive on the function pair.
 */
export function isAllowlisted(pair, entries) {
  for (const entry of entries ?? []) {
    if (normalizePath(entry.file) !== pair.a.file) continue;
    const allowed = new Set(entry.functions ?? []);
    if (allowed.has(pair.a.name) && allowed.has(pair.b.name)) return entry;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Gate evaluation
// ---------------------------------------------------------------------------

const isWithin = (fn, region) => fn.start >= region.start && fn.end <= region.end;

/**
 * Evaluates duplication pairs against production scope.
 * A pair is production only when BOTH sides are outside test regions and not
 * in `tests/` or `benches/` paths. `sources` maps normalized paths to contents
 * for every scanned file that appears in a pair; a missing source throws
 * (callers must fail closed instead of guessing the pair is production).
 */
export function evaluateDuplication({ pairs, sources, allowlist = [], violationThreshold = VIOLATION_THRESHOLD }) {
  const regionsByFile = new Map();
  const regionsFor = (file) => {
    if (!regionsByFile.has(file)) {
      const source = sources[file];
      if (typeof source !== 'string') {
        throw new Error(`missing source for scanned file: ${file}`);
      }
      regionsByFile.set(file, computeTestRegions(source));
    }
    return regionsByFile.get(file);
  };
  const sideScope = (fn) => {
    const classification = classifyPath(fn.file);
    if (classification === 'tests-dir') return 'test';
    if (classification === 'bench') return 'bench';
    const regions = regionsFor(fn.file);
    return regions.some((r) => isWithin(fn, r)) ? 'test' : 'production';
  };
  const toRecord = (pair) => ({ a: pair.a, b: pair.b, similarity: pair.similarity });

  const result = {
    counts: {
      pairsTotal: pairs.length,
      production: 0,
      violations: 0,
      allowlisted: 0,
      advisory: 0,
      testExcluded: 0,
      benchOutOfScope: 0,
    },
    violations: [],
    allowlisted: [],
    advisory: [],
    testExcluded: [],
    benchOutOfScope: [],
  };

  for (const pair of pairs) {
    const aScope = sideScope(pair.a);
    const bScope = sideScope(pair.b);
    if (aScope === 'bench' || bScope === 'bench') {
      result.counts.benchOutOfScope += 1;
      result.benchOutOfScope.push(toRecord(pair));
      continue;
    }
    if (aScope === 'test' || bScope === 'test') {
      result.counts.testExcluded += 1;
      result.testExcluded.push(toRecord(pair));
      continue;
    }
    result.counts.production += 1;
    if (pair.similarity >= violationThreshold) {
      const entry = isAllowlisted(pair, allowlist);
      if (entry) {
        result.counts.allowlisted += 1;
        result.allowlisted.push({ ...toRecord(pair), reason: entry.reason ?? '' });
      } else {
        result.counts.violations += 1;
        result.violations.push(toRecord(pair));
      }
    } else {
      result.counts.advisory += 1;
      result.advisory.push(toRecord(pair));
    }
  }
  return result;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

const formatPair = (pair) =>
  `  ${pair.a.file}:${pair.a.start}-${pair.a.end} ${pair.a.name} <-> ` +
  `${pair.b.file}:${pair.b.start}-${pair.b.end} ${pair.b.name}  ${(pair.similarity * 100).toFixed(2)}%`;

/**
 * CLI entry point. Returns the process exit code (0/1/2).
 * `opts` exists for tests: `{ root, similarityOutput, allowlist }` injects the
 * repo root, the raw similarity-rs stdout and the parsed allowlist so no
 * external tool is executed.
 */
export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');

  let similarityOutput = opts.similarityOutput;
  let toolVersion = 'injected';
  if (similarityOutput === undefined) {
    const versionRun = spawnSync(SIMILARITY_BIN, ['--version'], { encoding: 'utf8' });
    if (versionRun.error || versionRun.status !== 0) {
      console.error(`duplication gate: '${SIMILARITY_BIN}' not available`);
      return 2;
    }
    toolVersion = versionRun.stdout.trim();
    const run = spawnSync(SIMILARITY_BIN, SIMILARITY_ARGS, {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    });
    if (run.error || run.status !== 0) {
      console.error(`duplication gate: '${SIMILARITY_BIN} ${SIMILARITY_ARGS.join(' ')}' failed (status=${run.status})`);
      return 2;
    }
    similarityOutput = run.stdout ?? '';
  }

  let allowlist = opts.allowlist;
  if (allowlist === undefined) {
    const allowlistPath = path.join(root, 'duplication-allowlist.json');
    if (fs.existsSync(allowlistPath)) {
      try {
        allowlist = JSON.parse(fs.readFileSync(allowlistPath, 'utf8'));
      } catch (error) {
        console.error(`duplication gate: cannot parse duplication-allowlist.json: ${error.message}`);
        return 2;
      }
    } else {
      allowlist = { entries: [] };
    }
  }
  const allowlistEntries = allowlist?.entries ?? [];

  const pairs = parseSimilarityOutput(similarityOutput);
  const sources = {};
  for (const pair of pairs) {
    for (const side of [pair.a, pair.b]) {
      if (classifyPath(side.file) !== 'scan' || side.file in sources) continue;
      try {
        sources[side.file] = fs.readFileSync(path.join(root, ...side.file.split('/')), 'utf8');
      } catch (error) {
        console.error(`duplication gate: cannot read scanned file ${side.file}: ${error.message}`);
        return 2;
      }
    }
  }

  let result;
  try {
    result = evaluateDuplication({ pairs, sources, allowlist: allowlistEntries });
  } catch (error) {
    console.error(`duplication gate: evaluation failed: ${error.message}`);
    return 2;
  }

  const report = {
    generatedAt: new Date().toISOString(),
    tool: { name: SIMILARITY_BIN, version: toolVersion, args: [...SIMILARITY_ARGS] },
    violationThreshold: VIOLATION_THRESHOLD,
    counts: result.counts,
    violations: result.violations,
    allowlisted: result.allowlisted,
    advisory: result.advisory,
    testExcluded: result.testExcluded,
    benchOutOfScope: result.benchOutOfScope,
  };
  const reportPath = path.join(root, 'target', 'quality', 'duplication-report.json');
  try {
    fs.mkdirSync(path.dirname(reportPath), { recursive: true });
    fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  } catch (error) {
    console.error(`duplication gate: cannot write report ${reportPath}: ${error.message}`);
    return 2;
  }

  const { counts } = result;
  console.log(`duplication gate: violation threshold >= ${VIOLATION_THRESHOLD} (advisory 0.80-0.84)`);
  console.log(
    `  pairs: ${counts.pairsTotal} total = ${counts.production} production + ` +
      `${counts.testExcluded} test-excluded + ${counts.benchOutOfScope} bench (out of scope)`,
  );
  if (counts.allowlisted > 0) {
    console.log(`  allowlisted (exempt false positives): ${counts.allowlisted}`);
    for (const pair of result.allowlisted) console.log(`${formatPair(pair)}  [allowlisted]`);
  }
  if (counts.advisory > 0) {
    console.log(`  advisory (0.80-0.84, informational): ${counts.advisory}`);
  }
  if (counts.violations > 0) {
    console.log(`violations: ${counts.violations}`);
    for (const pair of result.violations) console.log(formatPair(pair));
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
