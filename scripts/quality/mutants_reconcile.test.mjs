import { mkdtempSync, existsSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMutantName, reconcile, selfTest, main } from './mutants_reconcile.mjs';

test('normalizeMutantName strips line/col, function suffix, and normalizes separators', () => {
  const n = normalizeMutantName('crates\\x\\src\\lib.rs:10:5: replace + with * in f');
  assert.equal(n.file, 'crates/x/src/lib.rs');
  assert.equal(n.op, 'replace + with *');
});

test('normalizeMutantName groups whole-function replacements into one class', () => {
  const n = normalizeMutantName('crates/x/src/lib.rs:3:1: replace big_fn with ()');
  assert.equal(n.op, 'replace <fn> with ()');
});

test('normalizeMutantName passes through names without a location prefix', () => {
  const n = normalizeMutantName('weird-name');
  assert.deepEqual(n, { file: 'weird-name', op: 'weird-name' });
});

test('reconcile classifies exact, equivalent, and new gaps', () => {
  const base = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with *' },
    { name: 'crates/y/src/other.rs:2:1: replace && with ||' },
  ];
  const cur = [
    { name: 'crates/x/src/lib.rs:14:5: replace + with *' },
    { name: 'crates/y/src/other.rs:2:1: replace && with ||' },
    { name: 'crates/z/src/new.rs:9:9: replace - with +' },
  ];
  const r = reconcile(base, cur);
  assert.equal(r.exact.length, 1, 'same file+line is exact');
  assert.equal(r.equivalent.length, 1, 'same class different line is equivalent');
  assert.equal(r.equivalent[0].matchedBaseline, base[0].name);
  assert.equal(r.newGap.length, 1);
  assert.equal(r.newGap[0].name, 'crates/z/src/new.rs:9:9: replace - with +');
});

test('reconcile exact matching is order-independent via slot swapping', () => {
  const base = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with *' },
    { name: 'crates/x/src/lib.rs:20:5: replace + with *' },
  ];
  const cur = [
    { name: 'crates/x/src/lib.rs:20:5: replace + with *' },
    { name: 'crates/x/src/lib.rs:10:5: replace + with *' },
  ];
  const r = reconcile(base, cur);
  assert.equal(r.exact.length, 2);
  assert.deepEqual(r.exact.map((e) => e.name).sort(), cur.map((c) => c.name).sort());
  assert.equal(r.equivalent.length + r.newGap.length, 0);
});

test('reconcile exhausts class slots before calling a repeat a new gap', () => {
  const base = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with *' },
    { name: 'crates/x/src/lib.rs:20:5: replace + with *' },
  ];
  const cur = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with *' },
    { name: 'crates/x/src/lib.rs:11:5: replace + with *' },
    { name: 'crates/x/src/lib.rs:12:5: replace + with *' },
  ];
  const r = reconcile(base, cur);
  assert.equal(r.exact.length, 1);
  assert.equal(r.equivalent.length, 1);
  assert.equal(r.newGap.length, 1);
});

test('selfTest accepts a realistic baseline', () => {
  const baseline = [
    { name: 'crates/a/src/lib.rs:10:5: replace + with * in add' },
    { name: 'crates/a/src/lib.rs:20:5: replace == with != in eq' },
    { name: 'crates/b/src/main.rs:3:1: replace run with ()' },
  ];
  const t = selfTest(baseline);
  assert.equal(t.ok, true, JSON.stringify(t));
});

test('main returns usage error without a report when arguments are missing', () => {
  assert.equal(main([]), 2);
});

test('main reconciles outcome-format fixtures and writes the report', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mutants-reconcile-test-'));
  const baselinePath = join(dir, 'baseline.json');
  const currentPath = join(dir, 'current.json');
  writeFileSync(baselinePath, JSON.stringify([
    { name: 'crates/x/src/lib.rs:10:5: replace + with *', file: 'crates/x/src/lib.rs', package: 'x', summary: 'MissedMutant' },
  ]));
  writeFileSync(currentPath, JSON.stringify({
    outcomes: [
      { scenario: { Mutant: { name: 'crates/x/src/lib.rs:10:5: replace + with *', file: 'crates/x/src/lib.rs', package: 'x' } }, summary: 'MissedMutant' },
      { scenario: { Mutant: { name: 'crates/x/src/lib.rs:30:5: replace + with *', file: 'crates/x/src/lib.rs', package: 'x' } }, summary: 'Timeout' },
      { scenario: { Mutant: { name: 'crates/x/src/lib.rs:40:5: replace + with *', file: 'crates/x/src/lib.rs', package: 'x' } }, summary: 'Survived' },
    ],
  }));
  const exit = main([baselinePath, currentPath]);
  assert.equal(exit, 0);
  const reportPath = join(process.cwd(), 'target', 'quality', 'mutants-reconcile.json');
  assert.ok(existsSync(reportPath), 'report written');
  const report = JSON.parse(readFileSync(reportPath, 'utf8'));
  assert.equal(report.baselineSurvivors, 1);
  assert.equal(report.currentSurvivors, 2, 'non-surviving outcomes are filtered before counting');
  assert.equal(report.counts.exact, 1);
  assert.equal(report.counts.newGap, 1);
});
