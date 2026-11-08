import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const src = readFileSync(join(wfDir, 'org-retrospective.js'), 'utf8');

function extractBlock(beginMarker, endMarker) {
  const begin = src.indexOf(beginMarker);
  const end = src.indexOf(endMarker, begin);
  assert.ok(begin > -1 && end > begin, `block ${JSON.stringify(beginMarker)} present`);
  return src.slice(begin, end);
}

test('meta declares the three retrospective phases in order', () => {
  const metaSrc = extractBlock('export const meta', "phase('Collect')").replace('export const meta', 'const meta');
  const meta = new Function(`${metaSrc}
return meta;`)();
  assert.equal(meta.name, 'org-retrospective');
  assert.deepEqual(meta.phases.map((p) => p.title), ['Collect', 'Analyze', 'Propose']);
});

test('collect reads the org feedback log and pins its analysis contract', () => {
  assert.ok(src.includes('.claude/org-feedback.md'), 'feedback log path pinned');
  assert.deepEqual(
    src.match(/required: \['total_entries', 'by_category', 'recurring_themes'\]/g),
    ["required: ['total_entries', 'by_category', 'recurring_themes']"],
    'collect schema required fields pinned',
  );
  for (const category of ['workflow', 'tooling', 'role_ambiguity', 'bottleneck', 'suggestion']) {
    assert.ok(
      src.includes(`${category}: { type: 'number' }`),
      `by_category declares ${category}`,
    );
  }
});

test('empty feedback short-circuits before the analyze phase', () => {
  const guard = src.indexOf('feedbackAnalysis.total_entries === 0');
  const analyze = src.indexOf("phase('Analyze')");
  assert.ok(guard > -1, 'zero-feedback guard exists');
  assert.ok(guard < analyze, 'guard returns before the analyze phase starts');
  assert.ok(src.includes("status: 'no-feedback'"), 'short-circuit returns the no-feedback status');
});

test('analyze schema forces ranked proposals with risk, effort, and impact', () => {
  assert.ok(
    src.includes("required: ['title', 'rationale', 'risk', 'effort', 'impact']"),
    'proposal item required fields pinned',
  );
  assert.equal(src.split("enum: ['low', 'medium', 'high']").length - 1, 2, 'risk and impact share the 3-level enum');
  assert.ok(src.includes("enum: ['trivial', 'small', 'medium', 'large']"), 'effort enum pinned');
  assert.ok(src.includes('meta_observations'), 'meta-observations about the feedback mechanism itself');
  assert.ok(src.includes('at most 5 proposals'), 'proposal count is bounded');
});

test('analyze consumes the collect output as structured input', () => {
  const analyze = src.indexOf("label: 'retro:analyze'");
  const interpolation = src.indexOf('${JSON.stringify(feedbackAnalysis, null, 2)}');
  assert.ok(analyze > -1, 'analyze agent present');
  assert.ok(interpolation > -1, 'feedback analysis serialized into the analyze prompt');
  assert.ok(interpolation > src.indexOf("label: 'retro:collect'"), 'collected data feeds the analyze prompt');
});

test('run result carries the retrospective contract fields', () => {
  assert.ok(src.includes("status: 'retrospective-complete'"), 'completion status pinned');
  for (const key of ['feedback_summary', 'proposals', 'meta_observations']) {
    assert.ok(src.includes(`${key}:`), `return includes ${key}`);
  }
});
