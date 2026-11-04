import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const SURVIVING = new Set(['MissedMutant', 'Timeout']);

export function normalizeMutantName(name) {
  const m = /^(.+?):\d+:\d+:\s+(.*)$/.exec(String(name));
  if (!m) return { file: String(name), op: String(name) };
  let op = m[2];
  op = op.replace(/\s+in\s+[^\s]+$/, '');
  op = op.replace(/^replace\s+\S+\s+with\s+\(\)$/, 'replace <fn> with ()');
  return { file: m[1].replace(/\\/g, '/'), op };
}

export function reconcile(baselineSurvivors, currentSurvivors) {
  const classes = new Map();
  for (const s of baselineSurvivors) {
    const key = classKey(s.name);
    if (!classes.has(key)) classes.set(key, { base: [], baseIdx: 0 });
    classes.get(key).base.push(s.name);
  }
  const exact = [];
  const equivalent = [];
  const newGap = [];
  const unmatched = [];
  for (const s of currentSurvivors) {
    const c = classes.get(classKey(s.name));
    if (c) {
      const at = c.base.indexOf(s.name, c.baseIdx);
      if (at !== -1) {
        [c.base[c.baseIdx], c.base[at]] = [c.base[at], c.base[c.baseIdx]];
        exact.push(s);
        c.baseIdx += 1;
        continue;
      }
    }
    unmatched.push(s);
  }
  for (const s of unmatched) {
    const c = classes.get(classKey(s.name));
    if (c && c.baseIdx < c.base.length) {
      equivalent.push({ ...s, matchedBaseline: c.base[c.baseIdx] });
      c.baseIdx += 1;
    } else {
      newGap.push(s);
    }
  }
  return { exact, equivalent, newGap };
}

const classKey = (name) => {
  const { file, op } = normalizeMutantName(name);
  return `${file}::${op}`;
};

export function selfTest(baselineSurvivors) {
  const base223 = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with * in f' },
    { name: 'crates/x/src/lib.rs:20:5: replace + with * in f' },
  ];
  const cur223 = [
    { name: 'crates/x/src/lib.rs:10:5: replace + with * in f' },
    { name: 'crates/x/src/lib.rs:22:5: replace + with * in g' },
    { name: 'crates/x/src/lib.rs:99:9: replace + with * in h' },
  ];
  const r223 = reconcile(base223, cur223);
  if (r223.exact.length !== 1 || r223.equivalent.length !== 1 || r223.newGap.length !== 1) {
    return { ok: false, failure: 'slot-consumption', detail: `exact=${r223.exact.length} equivalent=${r223.equivalent.length} newGap=${r223.newGap.length} (expected 1/1/1)` };
  }
  const cur232 = [
    { name: 'crates/x/src/lib.rs:5:9: replace + with * in g' },
    { name: 'crates/x/src/lib.rs:10:5: replace + with * in f' },
  ];
  const r232 = reconcile(base223, cur232);
  if (r232.exact.length !== 1 || !r232.exact.some((e) => e.name.includes(':10:'))) {
    return { ok: false, failure: 'order-independence', detail: `exact=${r232.exact.length} (${r232.exact.map((e) => e.name).join(', ')}) — the exact twin at :10 must be classified exact` };
  }
  const sanity = reconcile(baselineSurvivors, baselineSurvivors);
  if (sanity.newGap.length !== 0 || sanity.equivalent.length !== 0) {
    return { ok: false, failure: 'sanity', detail: `newGap=${sanity.newGap.length} equivalent=${sanity.equivalent.length} (expected all exact)` };
  }
  const someFile = baselineSurvivors[0]?.file ?? 'crates/x/src/lib.rs';
  const synthetic = [{ name: `${someFile}:1:1: replace ^^^never-an-op^^^ with ???`, file: someFile, package: '', summary: 'MissedMutant' }];
  const broken = reconcile(baselineSurvivors, synthetic);
  if (broken.newGap.length !== 1) {
    return { ok: false, failure: 'broken-variant', detail: `synthetic op classified as non-gap (newGap=${broken.newGap.length})` };
  }
  return { ok: true };
}

const readSurvivors = (p) => {
  const j = JSON.parse(fs.readFileSync(p, 'utf8'));
  if (Array.isArray(j)) return j;
  if (Array.isArray(j.survivors)) return j.survivors;
  if (!Array.isArray(j.outcomes)) throw new Error(`${p}: no outcomes array`);
  return j.outcomes
    .filter((o) => o?.scenario && typeof o.scenario === 'object' && o.scenario.Mutant && SURVIVING.has(o.summary))
    .map((o) => ({ name: o.scenario.Mutant.name, file: o.scenario.Mutant.file, package: o.scenario.Mutant.package, summary: o.summary }));
};

export function main(argv) {
  if (argv[0] === '--self-test') {
    const t = selfTest(readSurvivors(argv[1]));
    console.log(`mutants_reconcile self-test: ${t.ok ? 'OK' : 'FAIL'}`);
    if (!t.ok) console.error(JSON.stringify(t));
    return t.ok ? 0 : 2;
  }
  const [baselinePath, currentPath] = argv;
  if (!baselinePath || !currentPath) {
    console.error('usage: mutants_reconcile.mjs <baseline.json> <current.json> | --self-test <baseline.json>');
    return 2;
  }
  const baseline = readSurvivors(baselinePath);
  const current = readSurvivors(currentPath);
  const result = reconcile(baseline, current);
  const report = {
    generatedAt: new Date().toISOString(),
    baselineSurvivors: baseline.length,
    currentSurvivors: current.length,
    counts: {
      exact: result.exact.length,
      equivalent: result.equivalent.length,
      newGap: result.newGap.length,
    },
    newGap: result.newGap,
    equivalentSample: result.equivalent.slice(0, 200),
  };
  const out = path.resolve('target/quality/mutants-reconcile.json');
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`);
  console.log(`mutants_reconcile: baseline=${baseline.length} current=${current.length} -> exact=${report.counts.exact} equivalent=${report.counts.equivalent} NEW-GAP=${report.counts.newGap}`);
  for (const g of result.newGap.slice(0, 40)) console.error(`  [new-gap] ${g.name}`);
  if (result.newGap.length > 40) console.error(`  … and ${result.newGap.length - 40} more`);
  console.log(`report: ${out}`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exit(main(process.argv.slice(2)));
}
