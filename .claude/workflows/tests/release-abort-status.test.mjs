import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = 'const RELEASE_ABORT_STATUS';
const END = 'let precheckAbort';
const begin = src.indexOf(BEGIN);
const end = src.indexOf(END, begin);

let failures = 0;
function assert(cond, msg) {
  if (!cond) {
    failures += 1;
    console.error(`FAIL: ${msg}`);
  } else {
    console.log(`ok: ${msg}`);
  }
}

assert(begin >= 0, 'feature-pipeline.js contains RELEASE_ABORT_STATUS declaration');
assert(end > begin, 'feature-pipeline.js contains let precheckAbort after the span');

if (begin >= 0 && end > begin) {
  const block = src.slice(begin, end);
  const api = new Function(`${block}; return { resolveReleaseAbort, RELEASE_ABORT_STATUS };`)();

  let r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'no-tracked-changes' }, releaseResult: '' });
  assert(r.aborted === true, 'precheck.abort → aborted');
  assert(r.reason === 'no-tracked-changes', 'precheck reason propagated');

  r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'exclusion-only-changes' }, releaseResult: '' });
  assert(r.aborted === true && r.reason === 'exclusion-only-changes', 'exclusion-only abort propagated');

  r = api.resolveReleaseAbort({ precheck: { abort: true, reason: 'gitignored-only-artifacts' }, releaseResult: '' });
  assert(r.aborted === true && r.reason === 'gitignored-only-artifacts',
    'gitignored-only-artifacts abort propagated');

  r = api.resolveReleaseAbort({ precheck: { abort: false, reason: null }, releaseResult: 'PR_NUMBER=12' });
  assert(r.aborted === false, 'precheck ok → not aborted');

  r = api.resolveReleaseAbort({ precheck: null, releaseResult: 'ABORTED: no tracked changes' });
  assert(r.aborted === true, 'null precheck + ABORTED token in report → aborted (fallback)');
  assert(r.reason === 'agent-reported-abort', 'fallback reason is agent-reported-abort');

  r = api.resolveReleaseAbort({ releaseResult: 'branch feat/x\nPR_NUMBER=7' });
  assert(r.aborted === false, 'no precheck + normal report → not aborted');

  r = api.resolveReleaseAbort({ precheck: null, releaseResult: 'ABORTEDX typo' });
  assert(r.aborted === false, 'ABORTEDX is not an abort token (word boundary)');

  r = api.resolveReleaseAbort({ precheck: { abort: false, reason: null }, releaseResult: 'ABORTED: staging empty' });
  assert(r.aborted === true, 'precheck ok but agent ABORTED → aborted (fail-closed, double defense)');

  r = api.resolveReleaseAbort({});
  assert(r.aborted === false, 'empty args → not aborted (no signal, proceed)');
  r = api.resolveReleaseAbort(null);
  assert(r.aborted === false, 'null args → not aborted (no crash)');
  r = api.resolveReleaseAbort({ precheck: 'garbage', releaseResult: 42 });
  assert(r.aborted === false, 'malformed inputs → no crash');

  assert(api.RELEASE_ABORT_STATUS === 'empty-release-aborted', 'RELEASE_ABORT_STATUS is empty-release-aborted');
}

assert(src.includes("'empty-release-aborted'"), "workflow source contains 'empty-release-aborted' status literal");
const statusLine = src.match(/status: releaseAbort\.aborted \? RELEASE_ABORT_STATUS : \(allGo \? 'released' : 'review-rejected'\)/);
assert(statusLine !== null, 'return status branches: abort → empty-release-aborted, else released/review-rejected');

const phase6 = src.indexOf("phase('Release Review')");
const phase7 = src.indexOf("phase('Merge & Close')");
const phase8 = src.indexOf("phase('Self-Improve')");
const abortDecl = src.indexOf('const releaseAbort = resolveReleaseAbort(');
assert(abortDecl > 0 && phase6 > abortDecl, 'releaseAbort resolved before Phase 6');
assert(phase6 > 0 && phase7 > phase6 && phase8 > phase7, 'phase ordering intact (6 → 7 → 8)');

const between = src.slice(abortDecl, phase8);
assert(/if \(releaseAbort\.aborted\) \{[\s\S]*?\} else \{/.test(between), 'Phase 6-7 wrapped in if (releaseAbort.aborted) {} else {}');
assert(/Merge skipped/.test(src), 'merge-skip log retained');

const abortReturn = src.match(/releaseAbort\.aborted \? \{[\s\S]{0,600}snapshotBranch/);
assert(abortReturn !== null, 'abort return includes snapshotBranch (implementation is NOT lost — resumable)');
assert(/issue #66/.test(src.slice(abortDecl - 2000, abortDecl)) || /Issue #66/.test(src.slice(begin - 2000, begin)), 'issue #66 reference retained near the abort block (origin trail)');

if (failures > 0) {
  console.error(`\n${failures} test(s) FAILED`);
  process.exit(1);
}
console.log('\nAll release-abort-status tests passed');
