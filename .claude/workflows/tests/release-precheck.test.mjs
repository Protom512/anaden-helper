import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const wfPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'feature-pipeline.js');
const src = readFileSync(wfPath, 'utf8');

const BEGIN = 'const RELEASE_EXCLUDE_PATTERNS';
const END = "phase('Release')";
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

assert(begin >= 0, 'feature-pipeline.js contains RELEASE_EXCLUDE_PATTERNS declaration');
assert(end > begin, "feature-pipeline.js contains phase('Release') call after the span");

if (begin >= 0 && end > begin) {
  const block = src.slice(begin, end);
  const api = new Function(`${block}; return { evaluateReleasePrecheck, RELEASE_EXCLUDE_PATTERNS };`)();

  const T = (diff, porcelain, snap) => api.evaluateReleasePrecheck({
    diffNames: diff, porcelainLines: porcelain, hasSnapshotCommit: snap,
  });

  let r = T(['crates/foo/src/lib.rs', 'README.md'], [' M crates/foo/src/lib.rs'], false);
  assert(r.abort === false, 'tracked change present → no abort');
  assert(r.staged.length === 2, 'staged includes all non-excluded changed files');
  assert(r.reason === null, 'no abort reason when proceeding');

  r = T([], [], false);
  assert(r.abort === true, 'empty diff + empty porcelain + no snapshot → abort');
  assert(r.reason === 'no-tracked-changes', 'abort reason is no-tracked-changes');

  r = T([], ['?? probe_live.png'], false);
  assert(r.abort === true, 'untracked-only (excluded pattern) + no snapshot → abort');

  r = T(['.claude/workflows/feature-pipeline.js'], [' M .claude/workflows/feature-pipeline.js'], false);
  assert(r.abort === true, '.claude/-only change → abort (staging set empty)');
  assert(r.reason === 'exclusion-only-changes', 'abort reason is exclusion-only-changes');
  assert(r.excluded.length === 1, 'excluded lists the excluded file');

  r = T(['.omc/state/x.json', 'config.local'], [' M .omc/state/x.json'], false);
  assert(r.abort === true, '.omc/ + *.local only → abort');
  r = T(['x.lnk', '.claude.old/a.js', '.understand-anything/b.js'], [], false);
  assert(r.abort === true, '*.lnk / .claude.old/ / .understand-anything/ only → abort');

  r = T(['.omc/log.txt', 'src/main.rs'], [], false);
  assert(r.abort === false, 'mixed excluded + real change → proceed');
  assert(r.staged.length === 1 && r.staged[0] === 'src/main.rs', 'staged filters out excluded files');
  assert(r.excluded.length === 1, 'excluded keeps the excluded file');

  r = T([], [], true);
  assert(r.abort === false, 'empty diff BUT snapshot commit exists → proceed (R7 guard)');
  r = T(['.claude/workflows/x.js'], [' M .claude/workflows/x.js'], true);
  assert(r.abort === false, 'exclusion-only BUT snapshot commit exists → proceed');

  r = T([], ['M  src/staged.rs', ' M src/modified.rs'], false);
  assert(r.abort === false, 'porcelain-tracked changes count even when diffNames is empty/stale');
  assert(r.staged.includes('src/staged.rs') && r.staged.includes('src/modified.rs'),
    'porcelain staged and unstaged entries both included');

  r = T([], ['?? newfile.rs'], false);
  assert(r.abort === true, 'untracked-only porcelain entry does not prevent abort');

  r = T([], ['?? .omc/logs/run-1/gate.json'], false);
  assert(r.abort === true, 'gitignored artifact only (.omc/logs) → abort');
  assert(r.reason === 'gitignored-only-artifacts', 'reason is gitignored-only-artifacts (.omc artifact)');
  r = T([], ['?? target/debug/anaden.exe', '?? dist/bundle.js'], false);
  assert(r.abort === true && r.reason === 'gitignored-only-artifacts',
    'target/ + dist/ artifacts only → gitignored-only-artifacts');
  r = T(['.claude/workflows/feature-pipeline.js'], [' M .claude/workflows/feature-pipeline.js', '?? .omc/state/s.json'], false);
  assert(r.abort === true && r.reason === 'gitignored-only-artifacts',
    'staged=0 + untracked all gitignored artifacts → gitignored-only-artifacts (not exclusion-only)');
  r = T([], ['?? .omc/state/s.json', '?? src/real_change.rs'], false);
  assert(r.reason !== 'gitignored-only-artifacts' && r.abort === true,
    'untracked non-artifact file present → generic abort reason, not gitignored-only-artifacts');
  r = T([], ['?? .omc/logs/run/gate.json'], true);
  assert(r.abort === false, 'gitignored-only artifacts BUT snapshot commit → proceed (R7 guard precedence)');

  r = api.evaluateReleasePrecheck({});
  assert(r.abort === true, 'missing inputs default to abort (fail-closed, no empty release)');
  r = api.evaluateReleasePrecheck(null);
  assert(r.abort === true, 'null input → abort (no crash)');
  r = api.evaluateReleasePrecheck({ diffNames: 'not-an-array', porcelainLines: 5, hasSnapshotCommit: false });
  assert(r.abort === true, 'non-array inputs → abort (no crash)');
  r = T([42, null], [], false);
  assert(r.abort === true, 'non-string entries filtered out → still empty → abort');
  r = T(['src/a.rs', ''], [], false);
  assert(r.abort === false && r.staged.length === 1, 'empty-string filename filtered, real file kept');
}

if (failures > 0) {
  console.error(`\n${failures} test(s) FAILED`);
  process.exit(1);
}
console.log('\nAll release-precheck tests passed');
