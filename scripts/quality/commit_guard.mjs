import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

const GIT_FLAGS_WITH_VALUE = new Set([
  '-c',
  '-C',
  '--git-dir',
  '--work-tree',
  '--namespace',
  '--super-prefix',
]);

export function tokenizeCommand(command) {
  const tokens = [];
  let current = '';
  let quote = null;
  for (const ch of String(command)) {
    if (quote !== null) {
      if (ch === quote) quote = null;
      else current += ch;
      continue;
    }
    if (ch === '"' || ch === "'") {
      quote = ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (current !== '') {
        tokens.push(current);
        current = '';
      }
      continue;
    }
    current += ch;
  }
  if (current !== '') tokens.push(current);
  return tokens;
}

export function looksLikeGitCommit(command) {
  const tokens = tokenizeCommand(command);
  for (let i = 0; i < tokens.length; i += 1) {
    const name = tokens[i].replace(/\.exe$/i, '');
    if (name !== 'git') continue;
    let j = i + 1;
    while (j < tokens.length && tokens[j].startsWith('-')) {
      j += GIT_FLAGS_WITH_VALUE.has(tokens[j]) ? 2 : 1;
    }
    if (tokens[j] === 'commit') return true;
  }
  return false;
}

export function parseHookPayload(raw) {
  let payload;
  try {
    payload = JSON.parse(String(raw));
  } catch (error) {
    return { error: `hook payload is not valid JSON: ${error.message}` };
  }
  const command = payload?.tool_input?.command;
  if (typeof command !== 'string') {
    return { error: 'hook payload has no string tool_input.command' };
  }
  return { command };
}

export function gateArgs(root) {
  return [path.join(scriptDir, 'assert_tests_untouched.mjs'), '--root', root];
}

function readHookCommand() {
  let raw;
  try {
    raw = fs.readFileSync(0, 'utf8');
  } catch (error) {
    return { error: `cannot read hook payload from stdin: ${error.message}` };
  }
  return parseHookPayload(raw);
}

export function main(argv = [], opts = {}) {
  const commandResult = opts.command !== undefined
    ? { command: opts.command }
    : (opts.input !== undefined ? parseHookPayload(opts.input) : readHookCommand());
  if (commandResult.error) {
    console.error(`commit-guard: ${commandResult.error} — failing closed (exit 2)`);
    return 2;
  }
  if (!looksLikeGitCommit(commandResult.command)) return 0;

  let root = opts.root;
  if (root === undefined) {
    const rootRun = spawnSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' });
    if (rootRun.error || rootRun.status !== 0) return 0;
    root = String(rootRun.stdout ?? '').trim();
    if (root === '') return 0;
  }

  const runGate = opts.runGate
    ?? ((cwd) => spawnSync(
      process.execPath,
      gateArgs(cwd),
      { cwd, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 },
    ));
  const gate = runGate(root);
  if (gate.error) {
    console.error(`commit-guard: gate launch failed: ${gate.error.message ?? gate.error}`);
    console.error('commit blocked: tests-untouched guard failed (test region touched)');
    return 2;
  }
  if (gate.status !== 0) {
    if (gate.stdout) process.stdout.write(gate.stdout);
    if (gate.stderr) process.stderr.write(gate.stderr);
    console.error('commit blocked: tests-untouched guard failed (test region touched)');
    return 2;
  }
  console.log('commit-guard: git commit detected, tests-untouched guard passed');
  return 0;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
