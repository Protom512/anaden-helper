// "Tests untouched" guard for the anaden-helper workspace (Issue #214, Phase 0 / T3).
//
// Structural guarantee for the complexity-reduction campaign: no hunk of a git
// diff may touch test code — a #[cfg(test)] module/item region (including the
// attribute lines themselves) or a file under tests/. The protective core:
// existing test code may change only by provably format-only deltas or via an
// explicitly reviewed allowlist entry, and deletions of test files are never
// allowed. Three narrow allowances (Issue #272): (1) a whole-file diff whose
// old/new sources normalize equal (format-only: reindent, rewrap, and rustfmt
// trailing-comma normalization outside literals) is not a test change; (2) a
// brand-new file under tests/ (zero old lines) is the repo's additive pattern
// for new coverage; (3) residual violations in files listed in
// assert-tests-untouched-allowlist.json pass, and an entry matching no
// violation in the current diff fails as stale so the list cannot rot.
// Token edits (including inside string literals), mixed hunks, and deletions
// fail as before.
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
      current = { path: null, oldPath: null, isNew: false, isDeleted: false, isBinary: false, hunks: [] };
      files.push(current);
      inHunks = false;
      continue;
    }
    if (!current) continue;
    // Issue #239: hunk 本体内では `---`/`+++` ヘッダマッチを無効化する。
    // 削除行の内容が `-- select ...` 等で始まると `--- a/...` と誤判定し、
    // 行番号カウントが 1 つずれて test region 内削除を見逃す。
    if (!inHunks) {
      if (raw.startsWith('new file mode ')) current.isNew = true;
      if (raw.startsWith('deleted file mode ')) current.isDeleted = true;
      if (raw.startsWith('Binary files ') || raw.startsWith('GIT binary patch')) {
        current.isBinary = true;
        // Issue #245: binary 削除も path を復元 (oldPath から) して検出対象に。
        if (raw.includes(' and /dev/null differ')) current.path = current.oldPath ?? null;
        continue;
      }
      if (raw.startsWith('rename from ')) current.oldPath = normalizePath(raw.slice('rename from '.length));
      if (raw.startsWith('rename to ')) current.path = normalizePath(raw.slice('rename to '.length));
      // Issue #231: `--- a/<path>` は `+++` より先に現れるため保持 (削除ファイル対応)。
      const minusMatch = /^--- (?:a\/)?(.+)$/.exec(raw);
      if (minusMatch && minusMatch[1] !== '/dev/null') {
        current.oldPath = normalizePath(minusMatch[1]);
        continue;
      }
      const plusMatch = /^\+\+\+ (?:b\/)?(.+)$/.exec(raw);
      if (plusMatch && plusMatch[1] !== '/dev/null') {
        current.path = normalizePath(plusMatch[1]);
        continue;
      }
      if (plusMatch && plusMatch[1] === '/dev/null') {
        current.path = current.oldPath ?? null;
        continue;
      }
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
// Format-only normalization (Issue #272 v2)
// ---------------------------------------------------------------------------

/**
 * Normalizes Rust source for format-only comparison.
 * Outside literals and comments, drops all whitespace and every trailing
 * comma (a comma whose next non-whitespace character is `)`, `]`, or `}`).
 * String, byte-string, raw-string, and char literals keep their bytes
 * verbatim, so a change like "key zero" -> "keyzero" inside a literal is a
 * semantic edit, never a reformat. Comment text is kept with whitespace
 * dropped, so comment reflow passes while wording edits do not. Char-vs-
 * lifetime is a heuristic: 'x' / '\x' + closing quote reads as a char
 * literal, anything else as a lifetime, which needs no special handling.
 */
export function normalizeRustSource(src) {
  const text = String(src);
  const n = text.length;
  const out = [];
  const isIdentChar = (c) => /[A-Za-z0-9_]/.test(c);

  const emitCode = (chunk) => {
    const noTrailingCommas = chunk.replace(/,(?=\s*[)\]}])/g, '');
    out.push(noTrailingCommas.replace(/\s+/g, ''));
  };

  // Returns the index just past a cooked string's closing quote (`"` at start,
  // backslash escapes honored). Unterminated input consumes to the end, which
  // still compares deterministically on both sides.
  const scanQuoted = (start) => {
    let j = start + 1;
    while (j < n) {
      if (text[j] === '\\') { j += 2; continue; }
      if (text[j] === '"') return j + 1;
      j += 1;
    }
    return n;
  };

  let codeStart = 0;
  let i = 0;
  while (i < n) {
    const c = text[i];
    const prev = i > 0 ? text[i - 1] : '';
    if (c === '/' && text[i + 1] === '/') {
      const nl = text.indexOf('\n', i);
      const stop = nl === -1 ? n : nl;
      emitCode(text.slice(codeStart, i));
      out.push(text.slice(i + 2, stop).replace(/\s+/g, ''));
      codeStart = i = stop;
      continue;
    }
    if (c === '/' && text[i + 1] === '*') {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (text[j] === '/' && text[j + 1] === '*') { depth += 1; j += 2; }
        else if (text[j] === '*' && text[j + 1] === '/') { depth -= 1; j += 2; }
        else j += 1;
      }
      emitCode(text.slice(codeStart, i));
      out.push(text.slice(i + 2, j - 2).replace(/\s+/g, ''));
      codeStart = i = j;
      continue;
    }
    // Raw strings: r"..." / r#"..."# (##+) and the br-prefixed forms. An `r`
    // not followed by the quote shapes (e.g. the raw identifier `r#type`)
    // falls through as ordinary code.
    let rawPrefixLen = 0;
    if (!isIdentChar(prev)) {
      if (c === 'r' && (text[i + 1] === '"' || text[i + 1] === '#')) rawPrefixLen = 1;
      else if (c === 'b' && text[i + 1] === 'r' && (text[i + 2] === '"' || text[i + 2] === '#')) rawPrefixLen = 2;
    }
    if (rawPrefixLen > 0) {
      let j = i + rawPrefixLen;
      let hashes = 0;
      while (text[j] === '#') { hashes += 1; j += 1; }
      if (text[j] === '"') {
        const closing = '"' + '#'.repeat(hashes);
        const end = text.indexOf(closing, j + 1);
        const stop = end === -1 ? n : end;
        emitCode(text.slice(codeStart, i));
        out.push(text.slice(i, stop));
        codeStart = i = stop + closing.length;
        continue;
      }
    }
    if (c === '"' || (!isIdentChar(prev) && c === 'b' && text[i + 1] === '"')) {
      const stop = scanQuoted(c === '"' ? i : i + 1);
      emitCode(text.slice(codeStart, i));
      out.push(text.slice(i, stop));
      codeStart = i = stop;
      continue;
    }
    if (c === "'") {
      let j = i + 1;
      if (text[j] === '\\') j += 2;
      else j += 1;
      if (text[j] === "'") {
        emitCode(text.slice(codeStart, i));
        out.push(text.slice(i, j + 1));
        codeStart = i = j + 1;
        continue;
      }
    }
    i += 1;
  }
  emitCode(text.slice(codeStart, n));
  return out.join('');
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
    // A file whose old/new sources normalize equal is a pure reformat
    // (reindent, rewrap, rustfmt trailing-comma normalization) and moves no
    // token a test can observe (Issue #272). New files have no old side, so
    // tests/ additions still fall through to the checks below.
    const oldSrc = oldSources[file.path];
    const newSrc = newSources[file.path];
    if (
      typeof oldSrc === 'string'
      && typeof newSrc === 'string'
      && normalizeRustSource(oldSrc) === normalizeRustSource(newSrc)
    ) {
      continue;
    }
    if (classification === 'tests-dir') {
      // Brand-new tests/ files (zero old lines) are additive new coverage; the
      // guarded cases below are modification and deletion (Issue #272).
      if (oldLines.length === 0) continue;
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
 * `opts` exists for tests: `{ root, base, diffText, oldSources, newSources, allowlist }`
 * injects everything so no git command is executed.
 */
export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');
  const base = opts.base ?? argv[0] ?? 'HEAD';

  let allowlistEntries = opts.allowlist;
  if (allowlistEntries === undefined) {
    const allowlistPath = path.join(scriptDir, 'assert-tests-untouched-allowlist.json');
    let raw;
    try {
      raw = fs.readFileSync(allowlistPath, 'utf8');
    } catch (error) {
      console.error(`tests-untouched: cannot read assert-tests-untouched-allowlist.json: ${error.message}`);
      return 2;
    }
    try {
      allowlistEntries = JSON.parse(raw)?.entries ?? [];
    } catch (error) {
      console.error(`tests-untouched: cannot parse assert-tests-untouched-allowlist.json: ${error.message}`);
      return 2;
    }
  }

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
        // Issue #245: rename+modify の場合、old source は oldPath (元ファイル名)
        // から取得する。file.path は新しい名前のため git show が失敗する。
        const oldPath = file.oldPath && file.oldPath !== file.path ? file.oldPath : file.path;
        const show = spawnSync('git', ['-C', root, 'show', `${base}:${oldPath}`], { encoding: 'utf8' });
        if (show.error || show.status !== 0) {
          console.error(`tests-untouched: cannot read old version of ${oldPath} at ${base}`);
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

  const allowlistByFile = new Map(
    (allowlistEntries ?? []).map((entry) => [normalizePath(entry.file), entry]),
  );
  const remaining = [];
  const allowlistedFiles = new Set();
  for (const v of result.violations) {
    if (allowlistByFile.has(v.file)) {
      allowlistedFiles.add(v.file);
      continue;
    }
    remaining.push(v);
  }
  const stale = [...allowlistByFile.keys()].filter((f) => !allowlistedFiles.has(f));
  if (stale.length > 0) {
    console.error(`tests-untouched: stale allowlist entries (no matching violation in this diff):`);
    for (const f of stale) console.error(`  ${f}`);
  }
  if (remaining.length > 0) {
    console.error(`tests-untouched: ${remaining.length} hunk(s) touch test code (diff base: ${base})`);
    for (const v of remaining) {
      console.error(`  ${v.file}:${v.line} (${v.side}) ${v.reason}`);
    }
    return 1;
  }
  if (stale.length > 0) return 1;
  console.log(
    `tests-untouched: OK (${result.filesChecked} .rs file(s) in diff, no test code touched`
      + `${allowlistedFiles.size > 0 ? `, ${allowlistedFiles.size} allowlisted file(s)` : ''}; base=${base})`,
  );
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
