// cargo-mutants baseline gate (Issue #214, Phase 0 / T4).
//
// Gate decision (user-approved): a refactor phase passes when the workspace
// mutant run produces ZERO NEW surviving mutants relative to the recorded
// baseline. Existing survivors may persist; fixing them is welcome but not
// required. A mutant "survives" when tests still pass (MissedMutant) or hang
// (TimeoutMutant) — both mean the tests failed to kill it.
//
// Usage:
//   node scripts/quality/mutants_gate.mjs snapshot   # after a full run: record baseline
//   node scripts/quality/mutants_gate.mjs check      # after a later run: compare vs baseline
//
// Prerequisite run (overnight scale; .cargo/mutants.toml configures nextest +
// output under target/):
//   cargo mutants --workspace --in-place --jobserver-tasks 4
//
// Exit codes: 0 = pass, 1 = new survivors, 2 = missing/unusable inputs (fail-closed).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

const OUTCOMES_PATH = ['target', 'mutants.out', 'outcomes.json'];
const BASELINE_PATH = ['target', 'quality', 'mutants-baseline.json'];

const readJson = (root, relative) => {
  const full = path.join(root, ...relative);
  if (!fs.existsSync(full)) return null;
  try {
    return JSON.parse(fs.readFileSync(full, 'utf8'));
  } catch {
    return null;
  }
};

// ---------------------------------------------------------------------------
// Outcome parsing and comparison
// ---------------------------------------------------------------------------

// 実測 (cargo-mutants 27, 2026-10-18 baseline run): timeout の summary は
// "Timeout" ("TimeoutMutant" ではない)。Timeout = テストが mutant でハング =
// 殺せていないため survivor 扱い (新規 Timeout は回帰)。
const SURVIVING_SUMMARIES = new Set(['MissedMutant', 'Timeout']);

/**
 * Extracts survivors (Missed/Timeout mutants) and totals from a parsed
 * cargo-mutants outcomes.json. Note: the `success` flag is FALSE whenever any
 * mutant survives (cargo-mutants' own exit semantics), so run integrity is
 * validated from count consistency instead: the four outcome counts must sum
 * to total_mutants and match the per-entry outcomes actually present.
 * Throws on inconsistent data — a broken run must never be compared as clean.
 */
export function parseOutcomes(json) {
  if (!json || !Array.isArray(json.outcomes)) {
    throw new Error('outcomes.json has no outcomes array — malformed');
  }
  const survivors = [];
  let caught = 0;
  let unviable = 0;
  let missedCount = 0;
  let timeoutCount = 0;
  for (const entry of json.outcomes) {
    const scenario = entry?.scenario;
    const mutant = scenario && typeof scenario === 'object' ? scenario.Mutant : null;
    if (!mutant) continue; // Baseline entry
    if (entry.summary === 'MissedMutant') {
      missedCount += 1;
      survivors.push({
        name: mutant.name,
        package: mutant.package,
        file: mutant.file,
        summary: entry.summary,
      });
    } else if (entry.summary === 'Timeout') {
      timeoutCount += 1;
      survivors.push({
        name: mutant.name,
        package: mutant.package,
        file: mutant.file,
        summary: entry.summary,
      });
    } else if (entry.summary === 'CaughtMutant') {
      caught += 1;
    } else if (entry.summary === 'Unviable') {
      unviable += 1;
    }
  }
  const totals = {
    totalMutants: json.total_mutants ?? null,
    missed: json.missed ?? null,
    caught: json.caught ?? null,
    timeout: json.timeout ?? null,
    unviable: json.unviable ?? null,
  };
  const countedTotal = caught + unviable + survivors.length;
  if (
    totals.totalMutants !== null &&
    totals.totalMutants !== countedTotal
  ) {
    throw new Error(
      `outcomes.json inconsistent: total_mutants=${totals.totalMutants} but entries sum to ${countedTotal}`,
    );
  }
  // Issue #244: missed/timeout を個別に per-entry 数と照合 (両方非 null が前提)。
  if (totals.missed !== null && totals.missed !== missedCount) {
    throw new Error(
      `outcomes.json inconsistent: missed(${totals.missed}) != parsed MissedMutant entries (${missedCount})`,
    );
  }
  if (totals.timeout !== null && totals.timeout !== timeoutCount) {
    throw new Error(
      `outcomes.json inconsistent: timeout(${totals.timeout}) != parsed Timeout entries (${timeoutCount})`,
    );
  }
  if (totals.missed !== null && totals.timeout !== null && totals.missed + totals.timeout !== survivors.length) {
    throw new Error(
      `outcomes.json inconsistent: missed(${totals.missed})+timeout(${totals.timeout}) != parsed survivors (${survivors.length})`,
    );
  }
  return {
    survivors,
    totals,
    meta: {
      start_time: json.start_time ?? null,
      end_time: json.end_time ?? null,
      cargo_mutants_version: json.cargo_mutants_version ?? null,
    },
  };
}

/**
 * Set-diffs survivors by mutant name. `newSurvivors` (present now, absent in
 * baseline) fail the gate; `fixedSurvivors` are informational.
 */
export function compareSurvivors(baselineSurvivors, currentSurvivors) {
  const baselineNames = new Set(baselineSurvivors.map((s) => s.name));
  const currentNames = new Set(currentSurvivors.map((s) => s.name));
  return {
    newSurvivors: currentSurvivors.filter((s) => !baselineNames.has(s.name)),
    fixedSurvivors: baselineSurvivors.filter((s) => !currentNames.has(s.name)),
  };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function snapshot(root) {
  const outcomes = readJson(root, OUTCOMES_PATH);
  if (!outcomes) {
    console.error('mutants gate: target/mutants.out/outcomes.json not found — run cargo mutants first');
    return 2;
  }
  let parsed;
  try {
    parsed = parseOutcomes(outcomes);
  } catch (error) {
    console.error(`mutants gate: ${error.message}`);
    return 2;
  }
  const baseline = {
    generatedAt: new Date().toISOString(),
    sourceRun: parsed.meta,
    totals: parsed.totals,
    survivors: parsed.survivors,
  };
  const baselinePath = path.join(root, ...BASELINE_PATH);
  try {
    fs.mkdirSync(path.dirname(baselinePath), { recursive: true });
    fs.writeFileSync(baselinePath, `${JSON.stringify(baseline, null, 2)}\n`);
  } catch (error) {
    console.error(`mutants gate: cannot write baseline: ${error.message}`);
    return 2;
  }
  console.log(
    `mutants gate: baseline recorded — ${parsed.survivors.length} survivor(s) of ` +
      `${parsed.totals.totalMutants} mutants (${parsed.totals.missed} missed, ${parsed.totals.timeout} timeout)`,
  );
  return 0;
}

function check(root) {
  const baselineJson = readJson(root, BASELINE_PATH);
  if (!baselineJson || !Array.isArray(baselineJson.survivors)) {
    console.error('mutants gate: baseline (target/quality/mutants-baseline.json) missing or malformed — run `snapshot` first');
    return 2;
  }
  const outcomes = readJson(root, OUTCOMES_PATH);
  if (!outcomes) {
    console.error('mutants gate: target/mutants.out/outcomes.json not found — run cargo mutants first');
    return 2;
  }
  let parsed;
  try {
    parsed = parseOutcomes(outcomes);
  } catch (error) {
    console.error(`mutants gate: ${error.message}`);
    return 2;
  }
  const { newSurvivors, fixedSurvivors } = compareSurvivors(baselineJson.survivors, parsed.survivors);
  console.log(
    `mutants gate: current run — ${parsed.survivors.length} survivor(s) of ${parsed.totals.totalMutants} mutants; ` +
      `baseline had ${baselineJson.survivors.length}`,
  );
  if (fixedSurvivors.length > 0) {
    console.log(`  fixed vs baseline (informational): ${fixedSurvivors.length}`);
    for (const s of fixedSurvivors) console.log(`    ${s.name}`);
  }
  if (newSurvivors.length > 0) {
    console.error(`  NEW SURVIVORS (gate failure): ${newSurvivors.length}`);
    for (const s of newSurvivors) console.error(`    [${s.summary}] ${s.name}`);
    return 1;
  }
  console.log('mutants gate: OK — no new survivors vs baseline');
  return 0;
}

/**
 * CLI entry point. `mode` is `snapshot` or `check` (default: check).
 * `opts.root` overrides the repo root (tests).
 */
export function main(argv = [], opts = {}) {
  const root = opts.root ?? path.resolve(scriptDir, '..', '..');
  const mode = argv[0] ?? 'check';
  if (mode === 'snapshot') return snapshot(root);
  if (mode === 'check') return check(root);
  console.error(`mutants gate: unknown mode '${mode}' (use 'snapshot' or 'check')`);
  return 2;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  process.exit(main(process.argv.slice(2)));
}
