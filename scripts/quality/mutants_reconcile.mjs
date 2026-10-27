// Mutant-survivor reconciliation across refactors (Issue #214 / formal-methods-reconciler).
//
// Domain question: 「リファクタリングでテストカバレージを悪化させたか?」
// The mutants gate compares survivor NAMES (baseline vs current), but refactoring
// renames/splits functions, so name-keyed comparison reports renamed equivalents
// as "new survivors". This tool classifies each current survivor:
//
//   exact      — name identical to a baseline survivor (trivially not-new)
//   equivalent — same file + same normalized mutation op as a DISTINCT baseline
//                survivor (multiset/bipartite matching per (file, op) class —
//                the mutant is the same code defect observed through a renamed
//                or moved function)
//   new-gap    — no baseline survivor in its (file, op) class remains unmatched
//                → candidate genuine coverage regression, needs domain review
//
// Normalization: the mutant name "path:line:col: <op>" has its position and any
// trailing "in <QualifiedFn>" stripped; "<op>" like "replace != with ==" keeps
// its operands (the semantic defect), while "replace <fn> with ()" normalizes
// to the generic fn-deletion marker (function identity is exactly what a
// refactor changes, so it cannot be part of the equivalence key).
//
// The tool also runs built-in sanity + broken-variant checks (a reconciler that
// matches everything would be worthless):
//   --self-test  sanity: baseline vs itself → 0 new-gap;
//                broken: synthetic mutant with an op never present in its file
//                must be classified new-gap.
//
// Usage:
//   node scripts/quality/mutants_reconcile.mjs <baseline-outcomes.json> <current-outcomes.json>
//   node scripts/quality/mutants_reconcile.mjs --self-test <baseline-outcomes.json>
// Exit codes: 0 = classified (report written), 2 = input/self-test failure.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const SURVIVING = new Set(['MissedMutant', 'Timeout']);

/** Parses "file:line:col: rest" into {file, op} with the normalization above. */
export function normalizeMutantName(name) {
  const m = /^(.+?):\d+:\d+:\s+(.*)$/.exec(String(name));
  if (!m) return { file: String(name), op: String(name) };
  let op = m[2];
  // strip " in <fn-path>" (function qualifier contains no " in " itself)
  op = op.replace(/\s+in\s+[^\s]+$/, '');
  // fn-deletion mutants carry the (renamed) function in the op text → generalize
  op = op.replace(/^replace\s+\S+\s+with\s+\(\)$/, 'replace <fn> with ()');
  return { file: m[1].replace(/\\/g, '/'), op };
}

/**
 * Classifies current survivors against baseline survivors.
 * Matching is per (file, op) class: exact-name matches first, then injective
 * op-matching for the remainder (each baseline survivor matches at most once).
 */
export function reconcile(baselineSurvivors, currentSurvivors) {
  const byName = new Set(baselineSurvivors.map((s) => s.name));
  const classes = new Map(); // key -> { base: [names...], baseIdx: 0 }
  for (const s of baselineSurvivors) {
    const key = classKey(s.name);
    if (!classes.has(key)) classes.set(key, { base: [], baseIdx: 0 });
    classes.get(key).base.push(s.name);
  }
  const exact = [];
  const equivalent = [];
  const newGap = [];
  for (const s of currentSurvivors) {
    if (byName.has(s.name)) {
      exact.push(s);
      continue;
    }
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

/** Self-test: sanity (self-reconciliation → 0 new-gap) + broken variant. */
export function selfTest(baselineSurvivors) {
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
  if (Array.isArray(j)) return j; // plain survivor list
  if (Array.isArray(j.survivors)) return j.survivors; // mutants-baseline.json snapshot
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
