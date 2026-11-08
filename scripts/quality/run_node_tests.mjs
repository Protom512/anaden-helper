import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath, pathToFileURL } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

const EXCLUDED_DIRS = new Set([
  '.git',
  'node_modules',
  'target',
  '.omc',
  '.audit',
  '.anaden-run',
  '.claude.old',
]);

const normalizePath = (p) => String(p).replace(/\\/g, '/');

export function collectTestFiles(dir, root = dir) {
  const files = [];
  const pending = [dir];
  while (pending.length > 0) {
    const current = pending.pop();
    let entries;
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (EXCLUDED_DIRS.has(entry.name)) continue;
        if (fs.existsSync(path.join(full, '.git'))) continue;
        pending.push(full);
      } else if (entry.name.endsWith('.test.mjs')) {
        files.push(normalizePath(path.relative(root, full)));
      }
    }
  }
  return files.sort();
}

export function runSuite(files, opts = {}) {
  const node = opts.node ?? process.execPath;
  const cwd = opts.cwd ?? path.resolve(scriptDir, '..', '..');
  const maxBuffer = opts.maxBuffer ?? 64 * 1024 * 1024;
  const results = [];
  for (const file of files) {
    const started = performance.now();
    const run = spawnSync(node, ['--test', file], {
      cwd,
      encoding: 'buffer',
      maxBuffer,
      shell: false,
    });
    const status = run.error ? 'error' : run.status === 0 ? 'pass' : 'fail';
    const output = Buffer.concat([
      run.stdout ?? Buffer.alloc(0),
      run.stderr ?? Buffer.alloc(0),
    ]).toString('utf8');
    results.push({
      file,
      status,
      durationMs: Math.round(performance.now() - started),
      output,
    });
  }
  return results;
}

export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');
  const files = opts.files ?? collectTestFiles(root, root);
  if (files.length === 0) {
    console.error('run-node-tests: no *.test.mjs found under repo root');
    return 2;
  }
  console.log(`run-node-tests: ${files.length} file(s)`);
  const results = opts.results ?? runSuite(files, { cwd: root, node: opts.node });
  const failed = [];
  for (const r of results) {
    const mark = r.status === 'pass' ? 'PASS' : 'FAIL';
    console.log(`${mark} ${String(r.durationMs).padStart(5)}ms ${r.file}`);
    if (r.status !== 'pass') failed.push(r);
  }
  if (failed.length > 0) {
    console.error(`run-node-tests: ${failed.length}/${results.length} file(s) failed`);
    for (const r of failed) {
      console.error(`--- ${r.file} (status=${r.status}) ---`);
      const tail = (r.output ?? '').split(/\r?\n/).filter((l) => l.trim() !== '');
      console.error(tail.slice(-30).join('\n'));
    }
    return 1;
  }
  console.log(`run-node-tests: all green (${results.length} file(s))`);
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
