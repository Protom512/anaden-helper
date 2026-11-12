import test from 'node:test';
import assert from 'node:assert/strict';
import {
  redactionViolations,
  parseViewpointLedger,
  filterUntried,
  isBeyondVariance,
  detectPlateau,
  buildAuditorPrompt,
  parseAuditorReport,
} from '../plateau-auditor.js';

test('redactionViolations flags PR refs, code paths, and branch names', () => {
  assert.deepEqual(redactionViolations('PR #214 を再現'), ['PR #214 を再現'.match(/#\d{2,}/)[0]]);
  assert.ok(redactionViolations('crates/foo/src/lib.rs を直す').length === 1);
  assert.ok(redactionViolations('fix/parser-create-unique-index と同じ手').length === 1);
  assert.deepEqual(redactionViolations('冪等な再試行の待ち時間を短くする'), []);
});

test('parseViewpointLedger reads categories and one-line viewpoints', () => {
  const ledger = [
    '## 観測',
    '- 律速の所在を仮定せず、測定を先に置く',
    '',
    '## 判断',
    '- 文面が禁じていない手は測って決める',
  ].join('\n');
  const parsed = parseViewpointLedger(ledger);
  assert.equal(parsed.errors.length, 0);
  assert.equal(parsed.viewpoints.length, 2);
  assert.equal(parsed.viewpoints[0].category, '観測');
  assert.equal(parsed.viewpoints[1].category, '判断');
});

test('parseViewpointLedger collects errors instead of throwing', () => {
  const parsed = parseViewpointLedger('- #449 の再現手順\n- \n- 次の測定で UI 遅延を確認する\n');
  assert.equal(parsed.viewpoints.length, 1);
  const reasons = parsed.errors.map((e) => e.reason);
  assert.ok(reasons.includes('redaction-violation'));
  assert.ok(reasons.includes('empty-viewpoint'));
});

test('parseViewpointLedger rejects overly long viewpoints', () => {
  const parsed = parseViewpointLedger(`- ${'あ'.repeat(201)}`);
  assert.equal(parsed.viewpoints.length, 0);
  assert.equal(parsed.errors[0].reason, 'too-long');
});

test('filterUntried matches by normalized containment', () => {
  const viewpoints = [
    { category: '観測', text: '律速の所在を仮定せず、測定を先に置く' },
    { category: '判断', text: '文面が禁じていない手は測って決める' },
  ];
  const untried = filterUntried(viewpoints, ['測定を先に置く']);
  assert.equal(untried.length, 1);
  assert.equal(untried[0].text, '文面が禁じていない手は測って決める');
});

test('isBeyondVariance requires exceeding the variance band', () => {
  assert.equal(isBeyondVariance(121, 100, 20), true);
  assert.equal(isBeyondVariance(120, 100, 20), false);
  assert.equal(isBeyondVariance(100, 100, 0), false);
  assert.equal(isBeyondVariance(101, 100, 0), true);
  assert.equal(isBeyondVariance(Number.NaN, 100, 20), false);
});

test('detectPlateau only counts updates beyond the variance band', () => {
  const rising = [100, 200, 300, 400];
  assert.equal(detectPlateau(rising, 20, 3), false);
  const noisy = [100, 110, 115, 118, 120, 121];
  assert.equal(detectPlateau(noisy, 20, 3), true);
  const lateBreak = [100, 110, 115, 118, 120, 500];
  assert.equal(detectPlateau(lateBreak, 20, 3), false);
});

test('detectPlateau is fail-closed on malformed input', () => {
  assert.equal(detectPlateau([], 20, 3), false);
  assert.equal(detectPlateau([100, Number.NaN, 120], 20, 3), false);
  assert.equal(detectPlateau('100,110', 20, 3), false);
});

test('buildAuditorPrompt is fail-closed on an empty ledger', () => {
  assert.deepEqual(buildAuditorPrompt({ viewpoints: [] }), {
    ok: false,
    reason: 'empty-viewpoint-ledger',
  });
});

test('buildAuditorPrompt embeds untried viewpoints and the output contract', () => {
  const viewpoints = [{ category: '観測', text: '律速の所在を仮定せず、測定を先に置く' }];
  const built = buildAuditorPrompt({
    viewpoints,
    tried: ['ベンチ出力の要約を grep で削る'],
    plateauSummary: '直近4回の最高点更新がばらつき幅以内',
  });
  assert.equal(built.ok, true);
  assert.ok(built.prompt.includes('[観測] 律速の所在を仮定せず、測定を先に置く'));
  assert.ok(built.prompt.includes('UNTRIED: '));
  assert.ok(built.prompt.includes('NONE'));
});

test('buildAuditorPrompt reports when every viewpoint was already tried', () => {
  const viewpoints = [{ category: '観測', text: '測定を先に置く' }];
  const built = buildAuditorPrompt({ viewpoints, tried: ['測定を先に置く'] });
  assert.deepEqual(built, { ok: false, reason: 'no-untried-viewpoints' });
});

test('parseAuditorReport reads UNTRIED lines and NONE', () => {
  const report = 'UNTRIED: 測定を先に置く\nUNTRIED: 文面が禁じていない手は測って決める\n';
  assert.deepEqual(parseAuditorReport(report), {
    ok: true,
    items: ['測定を先に置く', '文面が禁じていない手は測って決める'],
  });
  assert.deepEqual(parseAuditorReport('NONE'), { ok: true, items: [] });
});

test('parseAuditorReport is fail-closed on unparseable or bloated output', () => {
  assert.deepEqual(parseAuditorReport('該当はありませんでした'), { ok: false, reason: 'unparseable' });
  assert.deepEqual(parseAuditorReport(''), { ok: false, reason: 'unparseable' });
  const bloated = Array.from({ length: 6 }, (_, i) => `UNTRIED: 観点${i}`).join('\n');
  assert.deepEqual(parseAuditorReport(bloated), { ok: false, reason: 'too-many-findings' });
});
