// TDD tests for Issue #78 (Task 3): review-gate Analyze phase must fetch the
// working-tree diff (R8 pattern from feature-pipeline.js) and inject it into
// reviewer prompts; MANDATORY checks must include `git --no-pager diff HEAD`
// with findings file/line cross-checked against the diff; functional reviewer
// findings schema must require the `file` field.
// Source-scan pattern — the workflow script is not importable.
//
// Run: node --test .claude/workflows/tests/review-gate-diff.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'review-gate.js');
const src = readFileSync(wfPath, 'utf8');
const helperSrc = readFileSync(join(dirname(wfPath), 'review-gate-diff.js'), 'utf8');

// Extract the full agent(...) call body containing a given label.
function reviewerBody(label) {
  const idx = src.indexOf(label);
  if (idx < 0) return null;
  const callStart = src.lastIndexOf('agent(', idx);
  const labelEnd = src.indexOf('label:', idx);
  return src.slice(callStart, labelEnd);
}

test('Analyze phase has a diff-fetch agent using git --no-pager diff HEAD', () => {
  assert.ok(
    src.includes('git --no-pager diff HEAD') || helperSrc.includes('git --no-pager diff HEAD'),
    'must run git --no-pager diff HEAD'
  );
  assert.ok(/fetch-diff/.test(src) || /fetch-diff/.test(src), 'diff fetch agent label');
});

test('diff fetch respects R8 caps: 28000 threshold, 24000 DIFF, [DIFF TRUNCATED]', () => {
  assert.ok(helperSrc.includes('28000'), '28000 char threshold');
  assert.ok(helperSrc.includes('24000'), '24000 DIFF cut');
  assert.ok(helperSrc.includes('[DIFF TRUNCATED]'), 'truncation marker');
});

test('diff text is injected into architecture, functional, maintainability reviewer prompts', () => {
  for (const label of ['review:architecture', 'review:functional', 'review:maintainability']) {
    const body = reviewerBody(label);
    assert.ok(body, `reviewer ${label} exists`);
    assert.ok(
      /withDiffContext\(/.test(body) || /\$\{\s*(REVIEW_DIFF|GATE_DIFF|diff)\s*\}/.test(body),
      `${label} prompt must inject the diff (withDiffContext or interpolation)`
    );
  }
});

test('MANDATORY checks in reviewer prompts include git --no-pager diff HEAD and file/line cross-check', () => {
  for (const label of ['review:architecture', 'review:functional', 'review:maintainability']) {
    const body = reviewerBody(label);
    assert.ok(body, `reviewer ${label} exists`);
    assert.ok(
      body.includes('git --no-pager diff HEAD'),
      `${label} MANDATORY checks must include git --no-pager diff HEAD`
    );
    assert.ok(
      /file\/line/.test(body) || /file.*line.*MUST.*match.*hunk/is.test(body),
      `${label} must require findings file/line match diff hunks`
    );
  }
});

test('functional reviewer findings schema includes file and line fields', () => {
  const idx = src.indexOf("label: 'review:functional'");
  const callStart = src.lastIndexOf('agent(', idx);
  const callEnd = src.indexOf('))', idx);
  const body = src.slice(callStart, callEnd);
  assert.ok(/file:\s*\{\s*type:\s*'string'/.test(body), 'functional findings need file field');
  assert.ok(/line:\s*\{\s*type:\s*'number'/.test(body), 'functional findings need line field');
});
