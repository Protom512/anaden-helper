import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { resolveGateLanes } from '../team-gate-protocol.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const src = readFileSync(join(wfDir, 'comprehensive-commit-gate.js'), 'utf8');

function extractBlock(beginMarker, endMarker) {
  const begin = src.indexOf(beginMarker);
  const end = src.indexOf(endMarker, begin);
  assert.ok(begin > -1 && end > begin, `block ${JSON.stringify(beginMarker)} present`);
  return src.slice(begin, end);
}

test('meta declares the two phases the script drives, in order', () => {
  const metaSrc = extractBlock('export const meta', 'const V_SCHEMA').replace('export const meta', 'const meta');
  const meta = new Function(`${metaSrc}
return meta;`)();
  assert.equal(meta.name, 'comprehensive-commit-gate');
  assert.deepEqual(meta.phases.map((p) => p.title), ['Non-Functional Review', 'Consensus']);
  const reviewIdx = src.indexOf("phase('Non-Functional Review')");
  const consensusIdx = src.indexOf("phase('Consensus')");
  assert.ok(reviewIdx > -1, "phase('Non-Functional Review') called");
  assert.ok(consensusIdx > reviewIdx, "phase('Consensus') called after review");
});

test('V_SCHEMA pins the reviewer verdict contract', () => {
  const schemaSrc = extractBlock('const V_SCHEMA', "phase('Non-Functional Review')");
  const V_SCHEMA = new Function(`${schemaSrc}
return V_SCHEMA;`)();
  assert.deepEqual(V_SCHEMA.required, ['verdict', 'dimension', 'findings', 'summary']);
  assert.deepEqual(V_SCHEMA.properties.verdict.enum, ['GO', 'NO-GO', 'CONDITIONAL']);
  assert.deepEqual(V_SCHEMA.properties.findings.items.required, ['severity', 'title', 'detail']);
  assert.ok(V_SCHEMA.properties.findings.items.properties.severity.enum.includes('critical'));
});

test('spawns exactly the canonical gate lanes as reviewers under V_SCHEMA', () => {
  const canonical = resolveGateLanes({ diffKind: 'code' }).lanes;
  const labels = [...src.matchAll(/label: 'review:([a-z]+)'/g)].map((m) => m[1]);
  assert.deepEqual(labels, canonical);
  assert.equal(src.split('schema: V_SCHEMA').length - 1, canonical.length, 'every reviewer is schema-forced');
});

test('consensus judge consumes all reviews and returns the final verdict', () => {
  assert.ok(src.includes('JSON.stringify(reviews'), 'reviews serialized into the judge prompt');
  assert.ok(src.includes("label: 'judge:consensus'"), 'consensus judge agent present');
  assert.ok(/final_verdict: \{ type: 'string', enum: \['GO', 'NO-GO', 'CONDITIONAL'\] \}/.test(src), 'final_verdict enum pinned');
  assert.ok(
    src.includes("'verdict_matrix', 'blocking_issues', 'judgment_calls', 'follow_up_items', 'final_verdict', 'commit_message'"),
    'judge schema required fields pinned'
  );
  assert.ok(/\breturn consensus\b/.test(src), 'workflow returns the consensus result');
});
