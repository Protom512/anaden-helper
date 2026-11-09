import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = join(wfDir, '..', '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

function loadInlineHelper() {
  const begin = fpSrc.indexOf('const RUN_TIMESTAMP_UNKNOWN');
  const end = fpSrc.indexOf('const DOCS_DIR_PREFIXES', begin);
  assert.ok(begin > -1 && end > begin, 'inline run-metadata-lookup block present');
  const inlineSrc = fpSrc.slice(begin, end);
  return new Function(
    `${inlineSrc.replace(/^\s*\/\/.*$/gm, '')}\nreturn { RUN_TIMESTAMP_UNKNOWN, buildRunTimestampIndex, matchRunByTimestamp };`
  )();
}

test('buildRunTimestampIndex indexes runTimestamp from run-metadata entries', () => {
  const { buildRunTimestampIndex } = loadInlineHelper();
  const index = buildRunTimestampIndex([
    { runId: 'run-1', metadata: { runId: 'run-1', runTimestamp: '2026-08-28T10:00:00+09:00' } },
    { runId: 'run-2', metadata: { runId: 'run-2', runTimestamp: '2026-08-28T11:00:00Z' } },
  ]);
  assert.equal(index.get('run-1').runTimestamp, '2026-08-28T10:00:00+09:00');
  assert.equal(index.get('run-2').runTimestamp, '2026-08-28T11:00:00Z');
});

test('fail-closed: missing metadata directory is classified unknown', () => {
  const { RUN_TIMESTAMP_UNKNOWN, buildRunTimestampIndex } = loadInlineHelper();
  const index = buildRunTimestampIndex([
    { runId: 'legacy-no-meta', metadata: null },
  ]);
  assert.equal(index.get('legacy-no-meta').runTimestamp, RUN_TIMESTAMP_UNKNOWN);
  assert.equal(RUN_TIMESTAMP_UNKNOWN, 'unknown');
});

test('fail-closed: metadata without runTimestamp field is unknown', () => {
  const { RUN_TIMESTAMP_UNKNOWN, buildRunTimestampIndex } = loadInlineHelper();
  const index = buildRunTimestampIndex([
    { runId: 'run-x', metadata: { runId: 'run-x' } },
    { runId: 'run-empty', metadata: {} },
  ]);
  assert.equal(index.get('run-x').runTimestamp, RUN_TIMESTAMP_UNKNOWN);
  assert.equal(index.get('run-empty').runTimestamp, RUN_TIMESTAMP_UNKNOWN);
});

test('fail-closed: non-string / malformed runTimestamp is unknown', () => {
  const { RUN_TIMESTAMP_UNKNOWN, buildRunTimestampIndex } = loadInlineHelper();
  const index = buildRunTimestampIndex([
    { runId: 'num', metadata: { runTimestamp: 123 } },
    { runId: 'null', metadata: { runTimestamp: null } },
    { runId: 'blank', metadata: { runTimestamp: '   ' } },
  ]);
  for (const id of ['num', 'null', 'blank']) {
    assert.equal(index.get(id).runTimestamp, RUN_TIMESTAMP_UNKNOWN, id);
  }
});

test('matchRunByTimestamp resolves runId for a known ISO timestamp', () => {
  const { buildRunTimestampIndex, matchRunByTimestamp } = loadInlineHelper();
  const ts = '2026-08-28T10:00:00+09:00';
  const index = buildRunTimestampIndex([
    { runId: 'run-1', metadata: { runTimestamp: ts } },
    { runId: 'run-2', metadata: { runTimestamp: '2026-08-28T11:00:00Z' } },
  ]);
  assert.equal(matchRunByTimestamp(index, ts), 'run-1');
});

test('matchRunByTimestamp returns null for unknown or unmatched timestamp', () => {
  const { buildRunTimestampIndex, matchRunByTimestamp } = loadInlineHelper();
  const index = buildRunTimestampIndex([
    { runId: 'legacy', metadata: null },
  ]);
  assert.equal(matchRunByTimestamp(index, 'unknown'), null);
  assert.equal(matchRunByTimestamp(index, '2099-01-01T00:00:00Z'), null);
  assert.equal(matchRunByTimestamp(index, null), null);
});

test('real-world: existing .omc/logs run dirs without run-metadata.json classify as unknown', (t) => {
  const { RUN_TIMESTAMP_UNKNOWN, buildRunTimestampIndex } = loadInlineHelper();
  const logsDir = join(repoRoot, '.omc', 'logs');
  if (!existsSync(logsDir)) return t.skip('machine-local .omc/logs runtime dir absent on fresh clones');
  assert.ok(existsSync(logsDir), '.omc/logs must exist');
  const runDirs = readdirSync(logsDir, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => d.name);
  assert.ok(runDirs.length > 0, 'at least one run dir expected');
  const entries = runDirs.map((runId) => {
    const metaPath = join(logsDir, runId, 'run-metadata.json');
    let metadata = null;
    if (existsSync(metaPath)) {
      try {
        metadata = JSON.parse(readFileSync(metaPath, 'utf8'));
      } catch {
        metadata = null;
      }
    }
    return { runId, metadata };
  });
  const index = buildRunTimestampIndex(entries);
  const unknownRuns = [...index.values()].filter((e) => e.runTimestamp === RUN_TIMESTAMP_UNKNOWN);
  assert.ok(
    unknownRuns.length > 0,
    'existing metadata-less log dirs (e.g. issue95-p008-verification) must classify as unknown'
  );
});

test('UC-1 wiring: runTimestamp/runId generated before phase(Coordinate)', () => {
  const coordIdx = fpSrc.indexOf("phase('Coordinate')");
  const tsIdx = fpSrc.indexOf('const runTimestamp');
  const idIdx = fpSrc.indexOf('const runId');
  assert.ok(coordIdx > -1, "phase('Coordinate') present");
  assert.ok(tsIdx > -1 && tsIdx < coordIdx, 'runTimestamp must be declared before Coordinate phase');
  assert.ok(idIdx > -1 && idIdx < coordIdx, 'runId must be declared before Coordinate phase');
});

test('UC-1 wiring: persister writes .omc/logs/{runId}/run-metadata.json', () => {
  assert.ok(
    fpSrc.includes('.omc/logs/${runId}/run-metadata.json'),
    'persister target path uses .omc/logs/{runId}/run-metadata.json'
  );
  assert.ok(
    fpSrc.includes("label: 'run:persist-run-metadata', phase: 'Request', model: 'sonnet'"),
    'persister uses sonnet in Request phase (P-005: haiku is Unknown-Model-400 on GLM backend)'
  );
});

function loadRunMetadataHelpers() {
  const begin = fpSrc.indexOf('const PIPELINE_PHASES');
  const end = fpSrc.indexOf("phase('Coordinate')", begin);
  assert.ok(begin > -1 && end > begin, 'inline run-metadata block present');
  const inlineSrc = fpSrc.slice(begin, end);
  return new Function(
    `${inlineSrc.replace(/^\s*\/\/.*$/gm, '')}\nreturn { isIso8601WithTimezone, buildRunMetadata };`
  )();
}

test('isIso8601WithTimezone accepts Z / +HH:MM / -HH:MM forms', () => {
  const { isIso8601WithTimezone } = loadRunMetadataHelpers();
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00Z'), true);
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00.123Z'), true);
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00+09:00'), true);
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00-05:30'), true);
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00.123456+09:00'), true);
});

test('isIso8601WithTimezone rejects TZ-less and malformed values', () => {
  const { isIso8601WithTimezone } = loadRunMetadataHelpers();
  assert.equal(isIso8601WithTimezone('2026-08-28T10:00:00'), false, 'naive local time rejected');
  assert.equal(isIso8601WithTimezone('2026-08-28 10:00:00Z'), false, 'space separator rejected');
  assert.equal(isIso8601WithTimezone('2026-08-28'), false, 'date-only rejected');
  assert.equal(isIso8601WithTimezone('20260828T100000Z'), false, 'basic format rejected');
  assert.equal(isIso8601WithTimezone(''), false);
  assert.equal(isIso8601WithTimezone(null), false);
  assert.equal(isIso8601WithTimezone(undefined), false);
  assert.equal(isIso8601WithTimezone(42), false);
});

test('buildRunMetadata normal case: full valid input yields no errors', () => {
  const { buildRunMetadata } = loadRunMetadataHelpers();
  const result = buildRunMetadata({
    runTimestamp: '2026-08-28T10:00:00+09:00',
    runId: 'run-1770000000000',
    issue: 97,
    title: '[Feature] pipeline 実行へ runTimestamp の記録・活用',
    phases: ['Coordinate', 'Request', 'Estimate'],
  });
  assert.deepEqual(result.errors, []);
  assert.deepEqual(result.value, {
    runTimestamp: '2026-08-28T10:00:00+09:00',
    runId: 'run-1770000000000',
    issue: 97,
    title: '[Feature] pipeline 実行へ runTimestamp の記録・活用',
    phases: ['Coordinate', 'Request', 'Estimate'],
  });
});

test('buildRunMetadata edge: missing fields are reported individually', () => {
  const { buildRunMetadata } = loadRunMetadataHelpers();
  const result = buildRunMetadata({});
  assert.ok(result.errors.includes('runTimestamp: missing'), 'runTimestamp missing reported');
  assert.ok(result.errors.includes('runId: missing'), 'runId missing reported');
  assert.ok(result.errors.includes('issue: missing'), 'issue missing reported');
  assert.ok(result.errors.includes('title: missing'), 'title missing reported');
  assert.ok(result.errors.includes('phases: missing'), 'phases missing reported');
  assert.equal(result.value, null, 'no value emitted on error');
});

test('buildRunMetadata edge: TZ-less timestamp rejected (fail-closed)', () => {
  const { buildRunMetadata } = loadRunMetadataHelpers();
  const result = buildRunMetadata({
    runTimestamp: '2026-08-28T10:00:00',
    runId: 'run-1',
    issue: 97,
    title: 't',
    phases: ['Coordinate'],
  });
  assert.ok(result.errors.some((e) => e.startsWith('runTimestamp:')));
  assert.equal(result.value, null);
});

test('buildRunMetadata edge: empty / non-array phases rejected', () => {
  const { buildRunMetadata } = loadRunMetadataHelpers();
  const base = {
    runTimestamp: '2026-08-28T10:00:00Z',
    runId: 'run-1',
    issue: 97,
    title: 't',
  };
  assert.ok(buildRunMetadata({ ...base, phases: [] }).errors.some((e) => e.startsWith('phases:')));
  assert.ok(buildRunMetadata({ ...base, phases: 'Coordinate' }).errors.some((e) => e.startsWith('phases:')));
  assert.ok(buildRunMetadata({ ...base, phases: null }).errors.some((e) => e.startsWith('phases:')));
});

test('buildRunMetadata edge: non-string runId / title rejected', () => {
  const { buildRunMetadata } = loadRunMetadataHelpers();
  const base = {
    runTimestamp: '2026-08-28T10:00:00Z',
    issue: 97,
    phases: ['Coordinate'],
  };
  assert.ok(buildRunMetadata({ ...base, runId: 123, title: 't' }).errors.some((e) => e.startsWith('runId:')));
  assert.ok(buildRunMetadata({ ...base, runId: 'run-1', title: null }).errors.some((e) => e.startsWith('title:')));
});

test('docs: pipeline-evidence-verification.md mentions runTimestamp as evidence requirement', (t) => {
  const docPath = join(repoRoot, '.claude', 'rules', 'pipeline-evidence-verification.md');
  if (!existsSync(docPath)) return t.skip('machine-local rules doc absent on fresh clones (gitignored .claude/rules/)');
  const doc = readFileSync(docPath, 'utf8');
  assert.ok(doc.includes('runTimestamp'), 'doc must mention runTimestamp');
  assert.ok(/runTimestamp/.test(doc.split('| 実行コマンド全文|')[1] ?? doc), 'sanity');
});
