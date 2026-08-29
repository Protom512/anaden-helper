// TDD tests for Issue #95 (P-008) T2: file ownership map + per-lane diff slice.
//
// gate-diff-kind.js additionally exposes:
//   ALL_LANES            — the 6 gate dimension keys of feature-pipeline.js
//   lanesForFile(path)   — owning lanes for one changed path (fail-closed:
//                          unknown path -> ALL_LANES, never under-review)
//   sliceDiffForLanes(diff, lanes) — per-lane unified-diff slice keyed by
//                          lane; files outside the lane's ownership are NOT
//                          injected into that lane
//
// Run: node --test .claude/workflows/tests/gate-lane-ownership.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { ALL_LANES, lanesForFile, sliceDiffForLanes } from '../gate-diff-kind.js';

// ── ALL_LANES ──

test('ALL_LANES covers the 6 gate dimensions of feature-pipeline.js', () => {
  assert.deepEqual([...ALL_LANES].sort(), [
    'extensibility',
    'governance',
    'integration',
    'performance',
    'reliability',
    'security',
  ]);
});

// ── ownership map ──

test('crates/** -> code lanes including extensibility (no silent under-review)', () => {
  const lanes = lanesForFile('crates/anaden-cli/src/main.rs');
  assert.ok(lanes.includes('reliability'));
  assert.ok(lanes.includes('performance'));
  assert.ok(lanes.includes('security'));
  assert.ok(lanes.includes('extensibility'));
});

test('scripts/ and .claude/** -> governance + integration', () => {
  assert.deepEqual(lanesForFile('scripts/run.sh'), ['governance', 'integration']);
  assert.deepEqual(lanesForFile('scripts/eval/harness.py'), ['governance', 'integration']);
  assert.deepEqual(lanesForFile('.claude/workflows/feature-pipeline.js'), ['governance', 'integration']);
});

test('unknown path / empty / non-string -> ALL_LANES (fail-closed)', () => {
  assert.deepEqual(lanesForFile('weird/path.bin'), [...ALL_LANES]);
  assert.deepEqual(lanesForFile(''), [...ALL_LANES]);
  assert.deepEqual(lanesForFile(null), [...ALL_LANES]);
  assert.deepEqual(lanesForFile(undefined), [...ALL_LANES]);
});

test('backslash paths normalized like isDocsPath', () => {
  assert.deepEqual(lanesForFile('scripts\\run.sh'), ['governance', 'integration']);
});

// ── sliceDiffForLanes ──

const SAMPLE_DIFF = [
  'diff --git a/crates/foo/src/lib.rs b/crates/foo/src/lib.rs',
  'index 111..222 100644',
  '--- a/crates/foo/src/lib.rs',
  '+++ b/crates/foo/src/lib.rs',
  '@@ -1,2 +1,2 @@',
  ' old',
  '+new',
  'diff --git a/scripts/run.sh b/scripts/run.sh',
  'index 333..444 100755',
  '--- a/scripts/run.sh',
  '+++ b/scripts/run.sh',
  '@@ -1 +1 @@',
  '-x',
  '+y',
  'diff --git a/README.md b/README.md',
  'index 555..666 100644',
  '--- a/README.md',
  '+++ b/README.md',
  '@@ -1 +1 @@',
  '-a',
  '+b',
].join('\n');

test('reliability slice contains crates hunk only (所有権外は注入しない)', () => {
  const slices = sliceDiffForLanes(SAMPLE_DIFF, ['reliability']);
  assert.ok(slices.reliability.includes('crates/foo/src/lib.rs'));
  assert.ok(!slices.reliability.includes('scripts/run.sh'));
  assert.ok(!slices.reliability.includes('README.md'));
});

test('governance slice contains scripts hunk, not crates', () => {
  const slices = sliceDiffForLanes(SAMPLE_DIFF, ['governance']);
  assert.ok(slices.governance.includes('scripts/run.sh'));
  assert.ok(!slices.governance.includes('crates/foo'));
});

test('lane with no owned files gets empty string (short-circuit signal)', () => {
  const onlyRust = [
    'diff --git a/crates/a/src/main.rs b/crates/a/src/main.rs',
    '--- a/crates/a/src/main.rs',
    '+++ b/crates/a/src/main.rs',
    '@@ -1 +1 @@',
    '-a',
    '+b',
  ].join('\n');
  const slices = sliceDiffForLanes(onlyRust, ['governance', 'reliability']);
  assert.equal(slices.governance, '');
  assert.ok(slices.reliability.length > 0);
});

test('unknown-path files injected into every requested lane (fail-closed)', () => {
  const diff = [
    'diff --git a/weird/path.bin b/weird/path.bin',
    '--- a/weird/path.bin',
    '+++ b/weird/path.bin',
    '@@ -1 +1 @@',
    '-a',
    '+b',
  ].join('\n');
  const slices = sliceDiffForLanes(diff, ['governance', 'security']);
  assert.ok(slices.governance.includes('weird/path.bin'));
  assert.ok(slices.security.includes('weird/path.bin'));
});

test('empty/invalid diff -> empty slice for every requested lane', () => {
  assert.deepEqual(sliceDiffForLanes('', ['reliability']), { reliability: '' });
  assert.deepEqual(sliceDiffForLanes('not a diff at all', ['governance']), { governance: '' });
  assert.deepEqual(sliceDiffForLanes(null, ['security']), { security: '' });
});

test('every requested lane appears as a key; ALL_LANES slicing keeps code lanes separated', () => {
  const slices = sliceDiffForLanes(SAMPLE_DIFF, ALL_LANES);
  for (const lane of ALL_LANES) {
    assert.ok(Object.hasOwn(slices, lane), `slice key missing: ${lane}`);
  }
  assert.ok(slices.extensibility.includes('crates/foo'));
  assert.ok(slices.integration.includes('scripts/run.sh'));
  assert.ok(!slices.security.includes('scripts/run.sh'));
});
