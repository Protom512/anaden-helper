// TDD tests for the empty-release abort return-status branch (Issue #66 Task 3).
// Adds 'empty-release-aborted' to the return status enum ('released'/'review-rejected')
// and skips Phase 6-7 (Release Review / Merge) on abort, going straight to Self-Improve.
// Same marker-block extraction pattern as team-verdict.test.mjs / release-precheck.test.mjs.
//
// Run: node .claude/workflows/tests/release-abort-status.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = '// [release-abort-status-begin]';
const END = '// [release-abort-status-end]';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END);

let failures = 0;
function assert(cond, msg) {
  if (!cond) {
    failures += 1;
    console.error(`FAIL: ${msg}`);
  } else {
    console.log(`ok: ${msg}`);
  }
}

assert(begin >= 0, 'feature-pipeline.js contains release-abort-status-begin marker');
assert(end > begin, 'feature-pipeline.js contains release-abort-status-end marker after begin');

if (begin >= 0 && end > begin) {
  const block = src.slice(begin + BEGIN.length, end);
  const api = new Function(`${block}; return { resolveReleaseAbort, RELEASE_ABORT_STATUS };`)();

  // ── JS precheck (Task 1) result is the primary, mechanical signal ──
  let r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'no-tracked-changes' }, releaseResult: '' });
  assert(r.aborted === true, 'precheck.abort → aborted');
  assert(r.reason === 'no-tracked-changes', 'precheck reason propagated');

  r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'exclusion-only-changes' }, releaseResult: '' });
  assert(r.aborted === true && r.reason === 'exclusion-only-changes', 'exclusion-only abort propagated');

  // P-003/step-3: gitignored-only-artifacts reason propagates through the abort path
  r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'gitignored-only-artifacts' }, releaseResult: '' });
  assert(r.aborted === true && r.reason === 'gitignored-only-artifacts',
    'gitignored-only-artifacts abort propagated');

  // ── precheck says proceed → not aborted regardless of prose ──
  r = api.resolveReleaseAbort({ precheck: { abort: false, reason: null }, releaseResult: 'PR_NUMBER=12' });
  assert(r.aborted === false, 'precheck ok → not aborted');

  // ── Task 1 not yet shipped (precheck null/undefined): fall back to the
  //    Release agent's ABORTED report token (Task 2 prompt guard) ──
  r = api.resolveReleaseAbort({ precheck: null, releaseResult: 'ABORTED: no tracked changes' });
  assert(r.aborted === true, 'null precheck + ABORTED token in report → aborted (fallback)');
  assert(r.reason === 'agent-reported-abort', 'fallback reason is agent-reported-abort');

  r = api.resolveReleaseAbort({ releaseResult: 'branch feat/x\nPR_NUMBER=7' });
  assert(r.aborted === false, 'no precheck + normal report → not aborted');

  // ABORTED token must be word-bounded (no false match on e.g. "not ABORTEDX")
  r = api.resolveReleaseAbort({ precheck: null, releaseResult: 'ABORTEDX typo' });
  assert(r.aborted === false, 'ABORTEDX is not an abort token (word boundary)');

  // precheck ok but agent still reported ABORTED → agent wins (fail-closed)
  r = api.resolveReleaseAbort({ precheck: { abort: false, reason: null }, releaseResult: 'ABORTED: staging empty' });
  assert(r.aborted === true, 'precheck ok but agent ABORTED → aborted (fail-closed, double defense)');

  // ── robustness: malformed inputs never crash; fail-closed on nothing ──
  r = api.resolveReleaseAbort({});
  assert(r.aborted === false, 'empty args → not aborted (no signal, proceed)');
  r = api.resolveReleaseAbort(null);
  assert(r.aborted === false, 'null args → not aborted (no crash)');
  r = api.resolveReleaseAbort({ precheck: 'garbage', releaseResult: 42 });
  assert(r.aborted === false, 'malformed inputs → no crash');

  // status constant referenced by the return statement
  assert(api.RELEASE_ABORT_STATUS === 'empty-release-aborted', 'RELEASE_ABORT_STATUS is empty-release-aborted');
}

// ── structural: workflow wiring (Phase 6-7 skip + status branch) ──
assert(src.includes("'empty-release-aborted'"), "workflow source contains 'empty-release-aborted' status literal");
const statusLine = src.match(/status: releaseAbort\.aborted \? RELEASE_ABORT_STATUS : \(allGo \? 'released' : 'review-rejected'\)/);
assert(statusLine !== null, 'return status branches: abort → empty-release-aborted, else released/review-rejected');

// abort branch must skip Release Review (Phase 6) and Merge (Phase 7)
const phase6 = src.indexOf('// ── Phase 6: Release Review');
const phase7 = src.indexOf('// ── Phase 7: Merge & Close');
const phase8 = src.indexOf('// ── Phase 8: Self-Improve');
const abortDecl = src.indexOf('const releaseAbort = resolveReleaseAbort(');
assert(abortDecl > 0 && phase6 > abortDecl, 'releaseAbort resolved before Phase 6');
assert(phase6 > 0 && phase7 > phase6 && phase8 > phase7, 'phase ordering intact (6 → 7 → 8)');

// Phase 6 body must be inside the not-aborted branch (guarded), and Phase 7's
// merge must not run on abort. Structural check: an `if (!releaseAbort.aborted)`
// guard exists between abortDecl and Phase 8.
const between = src.slice(abortDecl, phase8);
assert(/if \(!releaseAbort\.aborted\)/.test(between), 'Phase 6-7 wrapped in if (!releaseAbort.aborted)');
// abort path must log skip and still reach Self-Improve (Phase 8 exists after the guard)
assert(/Merge skipped/.test(src), 'merge-skip log retained');

// condition (2) of estimate approval: abort return keeps resumable info
// (current branch / diff summary reference) — structural check on the abort return.
const abortReturn = src.match(/releaseAbort\.aborted \? \{[\s\S]{0,600}snapshotBranch/);
assert(abortReturn !== null, 'abort return includes snapshotBranch (implementation is NOT lost — resumable)');
assert(/issue #66/.test(src.slice(abortDecl - 2000, abortDecl)) || /Issue #66/.test(src.slice(begin - 2000, begin)), 'JS comment references org-feedback.md issue #66 entries');

if (failures > 0) {
  console.error(`\n${failures} test(s) FAILED`);
  process.exit(1);
}
console.log('\nAll release-abort-status tests passed');
