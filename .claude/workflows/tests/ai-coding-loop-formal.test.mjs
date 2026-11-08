import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const src = readFileSync(join(wfDir, 'ai-coding-loop-formal.js'), 'utf8');

function extractBlock(beginMarker, endMarker) {
  const begin = src.indexOf(beginMarker);
  const end = src.indexOf(endMarker, begin);
  assert.ok(begin > -1 && end > begin, `block ${JSON.stringify(beginMarker)} present`);
  return src.slice(begin, end);
}

test('meta declares the six loop phases in execution order', () => {
  const metaSrc = extractBlock('export const meta', 'const FINDINGS_SCHEMA').replace('export const meta', 'const meta');
  const meta = new Function(`${metaSrc}
return meta;`)();
  assert.equal(meta.name, 'ai-coding-loop-formal');
  assert.deepEqual(meta.phases.map((p) => p.title), [
    'Explore',
    'Backlog',
    'Implement',
    'Evaluate',
    'Formalize',
    'Escalate',
  ]);
});

test('defaults pin maxIterations and the repo quality gate set as the loop metrics', () => {
  const slice = src.slice(src.indexOf('const goal = args?.goal'), src.indexOf("phase('Explore')"));
  const env = new Function(`const args = undefined;
${slice}
return { goal, constraints, maxIterations, perspectives, metrics, METRICS_BLOCK };`)();
  assert.ok(env.goal.length > 0, 'goal default is non-empty');
  assert.equal(env.maxIterations, 5);
  assert.equal(env.perspectives.length, 4);
  assert.deepEqual(env.metrics, [
    'cargo nextest run --workspace',
    'cargo clippy --all-targets -- -D warnings',
    'cargo fmt --all --check',
    'ast-grep scan',
  ]);
  assert.equal(env.METRICS_BLOCK, env.metrics.map((m) => `- ${m}`).join('\n'));
});

test('findings schema enforces severity-ranked findings with a summary', () => {
  const s = new Function(`${extractBlock('const FINDINGS_SCHEMA', 'const BACKLOG_SCHEMA')}
return FINDINGS_SCHEMA;`)();
  assert.deepEqual(s.required, ['findings', 'summary']);
  assert.deepEqual(s.properties.findings.items.required, ['title', 'detail', 'severity']);
  assert.ok(s.properties.findings.items.properties.severity.enum.includes('critical'));
});

test('impl schema is fail-closed: done requires commit context, blocked names a reason', () => {
  const s = new Function(`${extractBlock('const IMPL_SCHEMA', 'const EVAL_SCHEMA')}
return IMPL_SCHEMA;`)();
  assert.deepEqual(s.properties.status.enum, ['done', 'blocked']);
  assert.deepEqual(s.properties.blocked_reason.enum, ['context', 'permission', 'capability']);
  assert.deepEqual(s.required, ['status', 'summary']);
  assert.ok(s.properties.commit, 'commit field exists for green artifacts');
});

test('eval schema demands measured exit codes, not self-reports', () => {
  const s = new Function(`${extractBlock('const EVAL_SCHEMA', 'const RISK_SCHEMA')}
return EVAL_SCHEMA;`)();
  assert.deepEqual(s.required, ['green', 'commands']);
  assert.equal(s.properties.green.type, 'boolean');
  assert.deepEqual(s.properties.commands.items.required, ['cmd', 'exit_code']);
});

test('risk schema classifies into safe and dangerous with a suggested action', () => {
  const s = new Function(`${extractBlock('const RISK_SCHEMA', 'const goal = args?.goal')}
return RISK_SCHEMA;`)();
  assert.deepEqual(s.properties.risk.enum, ['safe', 'dangerous']);
  assert.deepEqual(s.required, ['risk', 'reasons', 'suggested_action']);
});

test('phases run in order and Formalize stays behind the formalize flag', () => {
  const order = ['Explore', 'Backlog', 'Implement', 'Evaluate', 'Escalate'].map((p) => src.indexOf(`phase('${p}')`));
  assert.ok(order.every((i) => i > -1), 'all mandatory phases present');
  assert.deepEqual(order, [...order].sort((a, b) => a - b), 'phases appear in execution order');
  const formalizeFlag = src.indexOf('if (args?.formalize)');
  const formalizePhase = src.indexOf("phase('Formalize')");
  assert.ok(formalizeFlag > -1, 'formalize flag exists');
  assert.ok(formalizePhase > formalizeFlag, 'Formalize phase is gated by args.formalize');
});

test('implementation agents commit with attribution and evaluation re-runs metrics', () => {
  assert.ok(src.includes('Co-Authored-By: glm 4.7 <noreply@zhipuai.cn>'), 'commit attribution pinned');
  assert.ok(src.includes('exit code と要約行を実測で返す'), 'evaluation demands measured output');
  assert.ok(src.includes('label: `fix:${item.id}`'), 'failed evaluation triggers a retry agent');
  assert.ok(src.includes("retry?.status === 'done'"), 'second evaluation only runs after a done retry');
  assert.ok(src.includes('.slice(0, maxIterations)'), 'backlog is capped at maxIterations');
});

test('run result carries the loop contract fields', () => {
  const ret = src.slice(src.indexOf('return {'));
  for (const key of ['goal', 'findings_count', 'backlog_total', 'results', 'formalized', 'risk', 'pr_url', 'escalated']) {
    assert.ok(new RegExp(`\\b${key}[,:]`).test(ret), `return includes ${key}`);
  }
  assert.ok(src.includes("escalated: risk?.risk === 'dangerous'"), 'dangerous classification escalates to human review');
});
