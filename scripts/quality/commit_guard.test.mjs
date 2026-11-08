import test from 'node:test';
import assert from 'node:assert/strict';
import { looksLikeGitCommit, parseHookPayload, main, tokenizeCommand, gateArgs } from './commit_guard.mjs';

test('tokenizeCommand strips quotes and splits on whitespace', () => {
  assert.deepEqual(
    tokenizeCommand("sh -c 'git commit -m x'"),
    ['sh', '-c', 'git commit -m x'],
  );
  assert.deepEqual(
    tokenizeCommand('git  -C  ../wt   commit'),
    ['git', '-C', '../wt', 'commit'],
  );
  assert.deepEqual(tokenizeCommand(''), []);
});

test('plain git commit is detected', () => {
  assert.equal(looksLikeGitCommit('git commit -m "fix"'), true);
  assert.equal(looksLikeGitCommit('git commit'), true);
});

test('git -C <path> commit is detected (issue #322 bypass form)', () => {
  assert.equal(looksLikeGitCommit('git -C ../wt-foo commit -m "..."'), true);
  assert.equal(looksLikeGitCommit('git -C "D:/repo with space" commit'), true);
});

test('git.exe / git.EXE are detected (issue #322 bypass form)', () => {
  assert.equal(looksLikeGitCommit('git.exe commit -m x'), true);
  assert.equal(looksLikeGitCommit('git.EXE commit -m x'), true);
});

test('env/command/sudo prefixed git commit is detected (issue #322 bypass form)', () => {
  assert.equal(looksLikeGitCommit('env git commit -m x'), true);
  assert.equal(looksLikeGitCommit('command git commit'), true);
  assert.equal(looksLikeGitCommit('sudo git commit -m x'), true);
});

test('chained and piped git commit is detected', () => {
  assert.equal(looksLikeGitCommit('cd sub && git commit -m x'), true);
  assert.equal(looksLikeGitCommit('make build | git commit'), true);
  assert.equal(looksLikeGitCommit('true; git commit'), true);
});

test('git global flags before commit are skipped', () => {
  assert.equal(looksLikeGitCommit('git -c color.ui=always commit'), true);
  assert.equal(looksLikeGitCommit('git --no-pager commit'), true);
  assert.equal(looksLikeGitCommit('git --git-dir=. --work-tree=. commit'), true);
  assert.equal(looksLikeGitCommit('git --paginate -C sub commit'), true);
});

test('non-commit git commands are not detected', () => {
  assert.equal(looksLikeGitCommit('git status'), false);
  assert.equal(looksLikeGitCommit('git log --oneline'), false);
  assert.equal(looksLikeGitCommit('git push origin master'), false);
  assert.equal(looksLikeGitCommit('git diff HEAD --stat'), false);
  assert.equal(looksLikeGitCommit('echo commit && git status'), false);
  assert.equal(looksLikeGitCommit('gitx commit'), false);
  assert.equal(looksLikeGitCommit('echo git'), false);
});

test('quoted prose containing "git commit" is not a false positive', () => {
  assert.equal(looksLikeGitCommit('echo "git commit" > notes.txt'), false);
  assert.equal(looksLikeGitCommit("printf '%s' 'git commit helper'"), false);
});

test('quoted full-command invocation stays undetected (known residual, pinned)', () => {
  assert.equal(looksLikeGitCommit("sh -c 'git commit -m x'"), false);
});

test('parseHookPayload extracts tool_input.command', () => {
  assert.deepEqual(
    parseHookPayload('{"tool_input":{"command":"git commit"}}'),
    { command: 'git commit' },
  );
});

test('parseHookPayload fails closed on malformed payloads', () => {
  assert.ok(parseHookPayload('not json').error.includes('not valid JSON'));
  assert.ok(parseHookPayload('{"tool_input":{}}').error.includes('tool_input.command'));
  assert.ok(parseHookPayload('{"tool_input":{"command":42}}').error.includes('tool_input.command'));
});

const PASS_GATE = () => ({ status: 0, stdout: 'gate ok', stderr: '' });

test('gateArgs pins the --root contract with the gate script', () => {
  const args = gateArgs('D:/repo');
  assert.equal(args[0].endsWith('assert_tests_untouched.mjs'), true);
  assert.deepEqual(args.slice(1), ['--root', 'D:/repo']);
});

test('main passes a non-commit command without invoking the gate', () => {
  const code = main([], { command: 'git status', runGate: () => { throw new Error('gate must not run'); } });
  assert.equal(code, 0);
});

test('main runs the gate for a git commit and passes on green', () => {
  let calledWith = null;
  const code = main([], {
    command: 'git -C ../wt commit -m x',
    root: 'D:/repo',
    runGate: (cwd) => {
      calledWith = cwd;
      return { status: 0, stdout: '', stderr: '' };
    },
  });
  assert.equal(code, 0);
  assert.equal(calledWith, 'D:/repo');
});

test('main blocks with exit 2 when the gate fails', () => {
  const code = main([], {
    command: 'git commit -m x',
    root: 'D:/repo',
    runGate: () => ({ status: 1, stdout: 'violation list', stderr: '' }),
  });
  assert.equal(code, 2);
});

test('main blocks with exit 2 when the gate cannot launch', () => {
  const code = main([], {
    command: 'git commit -m x',
    root: 'D:/repo',
    runGate: () => ({ error: new Error('ENOENT'), status: null, stdout: '', stderr: '' }),
  });
  assert.equal(code, 2);
});

test('main fails closed on malformed hook payload', () => {
  const code = main([], {
    input: '{broken',
    runGate: () => { throw new Error('gate must not run'); },
  });
  assert.equal(code, 2);
});

test('main fails closed when payload lacks the command field', () => {
  const code = main([], {
    input: '{"tool_input":{}}',
    runGate: () => { throw new Error('gate must not run'); },
  });
  assert.equal(code, 2);
});
