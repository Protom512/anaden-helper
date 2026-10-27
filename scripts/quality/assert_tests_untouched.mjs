// "Tests untouched" guard for the anaden-helper workspace (Issue #214, Phase 0 / T3).
//
// Structural guarantee for the complexity-reduction campaign: no hunk of a git
// diff may touch test code — a #[cfg(test)] module/item region (including the
// attribute lines themselves) or any file under tests/. Additions count as
// touches: the campaign contract is "not a single line of test code changes".
//
// Usage:
//   node scripts/quality/assert_tests_untouched.mjs [base]
//     base omitted -> diff HEAD (staged + unstaged vs HEAD)
//     base given   -> diff <base> (e.g. master, HEAD~3)
//
// Exit codes: 0 = no test code touched, 1 = violations, 2 = tool/parse error
// (fail-closed). An empty diff passes trivially — this is a mechanical
// pre-commit check, not a review gate.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { computeTestRegions, classifyPath } from './complexity_gate.mjs';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const normalizePath = (p) => String(p).replace(/\\/g, '/');

// ---------------------------------------------------------------------------
// Unified diff parsing
// ---------------------------------------------------------------------------

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * Parses a unified diff into file entries with per-side changed line numbers:
 * `[{ path, isNew, isDeleted, hunks: [{ oldLines: [n..], newLines: [n..] }] }]`.
 * Context lines advance both cursors; `\ No newline` markers are skipped.
 */
export function parseDiff(diffText) {
  const files = [];
  let current = null;
  let inHunks = false;
  let oldCursor = 0;
  let newCursor = 0;
  for (const raw of String(diffText).split(/\r?\n/)) {
    if (raw.startsWith('diff --git ')) {
      current = { path: null, isNew: false, isDeleted: false, hunks: [] };
      files.push(current);
      inHunks = false;
      continue;
    }
    if (!current) continue;
    if (raw.startsWith('new file mode ')) current.isNew = true;
    if (raw.startsWith('deleted file mode ')) current.isDeleted = true;
    if (raw.startsWith('Binary files ') || raw.startsWith('GIT binary patch')) inHunks = false;
    const plusMatch = /^\+\+\+ (?:b\/)?(.+)$/.exec(raw);
    if (plusMatch && plusMatch[1] !== '/dev/null') {
      current.path = normalizePath(plusMatch[1]);
      continue;
    }
    const hunkMatch = HUNK_HEADER.exec(raw);
    if (hunkMatch) {
      current.hunks.push({ oldLines: [], newLines: [] });
      oldCursor = Number(hunkMatch[1]);
      newCursor = Number(hunkMatch[3]);
      inHunks = true;
      continue;
    }
    if (!inHunks || current.hunks.length === 0) continue;
    const hunk = current.hunks[current.hunks.length - 1];
    if (raw.startsWith('-')) {
      hunk.oldLines.push(oldCursor);
      oldCursor += 1;
    } else if (raw.startsWith('+')) {
      hunk.newLines.push(newCursor);
      newCursor += 1;
    } else if (raw.startsWith(' ')) {
      oldCursor += 1;
      newCursor += 1;
    } // '\\ No newline at end of file' and anything else: skip
  }
  return files.filter((f) => f.path !== null);
}

// ---------------------------------------------------------------------------
// Region checks
// ---------------------------------------------------------------------------

/**
 * Evaluates a parsed diff against old/new sources.
 * Returns `{ violations: [{file, side, line, reason}], filesChecked }`.
 * Throws when a needed source is missing (callers fail closed).
 */
export function evaluateUntouched({ files, oldSources = {}, newSources = {} }) {
  const violations = [];
  let filesChecked = 0;
  const sourceFor = (map, file, side) => {
    const src = map[file];
    if (typeof src !== 'string') throw new Error(`missing ${side} source for ${file}`);
    return src;
  };
  for (const file of files) {
    if (!file.path.endsWith('.rs')) continue;
    const oldLines = file.hunks.flatMap((h) => h.oldLines);
    const newLines = file.hunks.flatMap((h) => h.newLines);
    const classification = classifyPath(file.path);
    filesChecked += 1;
    if (classification === 'tests-dir') {
      for (const line of newLines) {
        violations.push({ file: file.path, side: 'new', line, reason: 'tests/ directory file changed' });
      }
      for (const line of oldLines) {
        violations.push({ file: file.path, side: 'old', line, reason: 'tests/ directory file changed' });
      }
      continue;
    }
    if (classification !== 'scan') continue; // benches are out of test scope
    if (newLines.length > 0) {
      const regions = computeTestRegions(sourceFor(newSources, file.path, 'new'));
      for (const line of newLines) {
        const hit = regions.find((r) => line >= r.attrStart && line <= r.end);
        if (hit) violations.push({ file: file.path, side: 'new', line, reason: `inside ${hit.kind} test region [${hit.attrStart}..${hit.end}]` });
      }
    }
    if (oldLines.length > 0) {
      const regions = computeTestRegions(sourceFor(oldSources, file.path, 'old'));
      for (const line of oldLines) {
        const hit = regions.find((r) => line >= r.attrStart && line <= r.end);
        if (hit) violations.push({ file: file.path, side: 'old', line, reason: `inside ${hit.kind} test region [${hit.attrStart}..${hit.end}]` });
      }
    }
  }
  return { violations, filesChecked };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/**
 * CLI entry point. Returns the process exit code (0/1/2).
 * `opts` exists for tests: `{ root, base, diffText, oldSources, newSources }`
 * injects everything so no git command is executed.
 */
export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');
  const base = opts.base ?? argv[0] ?? 'HEAD';

  let diffText = opts.diffText;
  let oldSources = opts.oldSources;
  let newSources = opts.newSources;
  if (diffText === undefined) {
    const diffRun = spawnSync('git', ['-C', root, 'diff', '--unified=0', base, '--'], {
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    });
    if (diffRun.error || diffRun.status !== 0) {
      console.error(`tests-untouched: git diff ${base} failed (status=${diffRun.status}) ${(diffRun.stderr ?? '').trim()}`);
      return 2;
    }
    diffText = diffRun.stdout ?? '';
    oldSources = {};
    newSources = {};
    for (const file of parseDiff(diffText)) {
      if (!file.path.endsWith('.rs')) continue;
      if (file.hunks.some((h) => h.oldLines.length > 0)) {
        const show = spawnSync('git', ['-C', root, 'show', `${base}:${file.path}`], { encoding: 'utf8' });
        if (show.error || show.status !== 0) {
          console.error(`tests-untouched: cannot read old version of ${file.path} at ${base}`);
          return 2;
        }
        oldSources[file.path] = show.stdout ?? '';
      }
      if (file.hunks.some((h) => h.newLines.length > 0)) {
        try {
          newSources[file.path] = fs.readFileSync(path.join(root, ...file.path.split('/')), 'utf8');
        } catch (error) {
          console.error(`tests-untouched: cannot read ${file.path}: ${error.message}`);
          return 2;
        }
      }
    }
  }

  let files;
  let result;
  try {
    files = parseDiff(diffText);
    result = evaluateUntouched({ files, oldSources, newSources });
  } catch (error) {
    console.error(`tests-untouched: evaluation failed: ${error.message}`);
    return 2;
  }

  if (result.violations.length > 0) {
    console.error(`tests-untouched: ${result.violations.length} hunk(s) touch test code (diff base: ${base})`);
    for (const v of result.violations) {
      console.error(`  ${v.file}:${v.line} (${v.side}) ${v.reason}`);
    }
    return 1;
  }
  console.log(`tests-untouched: OK (${result.filesChecked} .rs file(s) in diff, no test code touched; base=${base})`);
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
