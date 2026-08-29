// TDD tests for Issue #91 (P-007) T5: wire the intent<->fact contrast check
// (review-gate-contrast.js testContrast) into the Commit Gate reviewer prompts
// in feature-pipeline.js, alongside FEEDBACK_INSTRUCTION, and document
// CONDITIONAL semantics (mismatch → CONDITIONAL with explicit override
// justification) in the QC/consensus rule text.
//
// Source-scan based: feature-pipeline.js is a Workflow script (not importable).
//
// Run: node --test .claude/workflows/tests/gate-contrast-wiring.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { testContrast } from '../review-gate-contrast.js';

const wfDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const fpSrc = readFileSync(join(wfDir, 'feature-pipeline.js'), 'utf8');

test('feature-pipeline: contrast check runs before reviewer prompts (fail-closed short-circuit preserved)', () => {
  const contrastAt = fpSrc.indexOf('const gateContrast =');
  const instrAt = fpSrc.indexOf('const FEEDBACK_INSTRUCTION');
  const dimsAt = fpSrc.indexOf('const GATE_DIMENSIONS =');
  const activeDimsAt = fpSrc.indexOf('const ACTIVE_GATE_DIMENSIONS =');
  assert.ok(contrastAt > 0, 'contrast block exists');
  assert.ok(instrAt > 0 && contrastAt < instrAt, 'contrast computed before FEEDBACK_INSTRUCTION');
  // P-008 (Issue #95): the executed lane set is ACTIVE_GATE_DIMENSIONS (derived
  // from GATE_DIMENSIONS via the diff-kind filter); contrast must precede both.
  assert.ok(dimsAt > 0 && contrastAt < dimsAt, 'contrast computed before GATE_DIMENSIONS');
  assert.ok(activeDimsAt > 0, 'ACTIVE_GATE_DIMENSIONS lane set exists (P-008)');
  assert.ok(contrastAt < activeDimsAt, 'contrast computed before ACTIVE_GATE_DIMENSIONS');
  const failAt = fpSrc.indexOf("mode === 'fail-closed'");
  assert.ok(failAt > 0 && failAt < contrastAt, 'diff fail-closed short-circuit still precedes contrast');
});

test('feature-pipeline: contrast uses ticket title + estimate.tasks files + GATE_DIFF', () => {
  const block = fpSrc.slice(fpSrc.indexOf('gateContrast'), fpSrc.indexOf('gateContrast') + 900);
  assert.ok(block.includes('testContrast'), 'wired to T4 pure helper');
  assert.ok(/ticket\.title/.test(block), 'ticket title passed');
  assert.ok(/estimate\.tasks/.test(block) || /taskFiles/.test(block), 'estimate task files passed');
  assert.ok(/GATE_DIFF/.test(block), 'actual injected diff is the fact source');
});

test('feature-pipeline: contrast result injected into FEEDBACK_INSTRUCTION with CONDITIONAL-override instruction', () => {
  const instrAt = fpSrc.indexOf('const FEEDBACK_INSTRUCTION');
  const instr = fpSrc.slice(instrAt, instrAt + 6000);
  assert.ok(instr.includes('${GATE_CONTRAST_REPORT}'), 'contrast report referenced in reviewer instruction');
  // The CONDITIONAL/override semantics live in GATE_CONTRAST_REPORT's mismatch branch.
  const reportAt = fpSrc.indexOf('const GATE_CONTRAST_REPORT');
  const report = fpSrc.slice(reportAt, reportAt + 1400);
  assert.ok(report.includes('MISMATCH'), 'mismatch branch exists');
  assert.ok(/CONDITIONAL/.test(report), 'mismatch → CONDITIONAL semantics stated');
  assert.ok(/override/i.test(report), 'explicit override justification required');
});

test('feature-pipeline: consensus judge rules document CONDITIONAL override for contrast mismatch', () => {
  const judgeAt = fpSrc.indexOf('`CONSENSUS JUDGE.');
  assert.ok(judgeAt > 0, 'judge prompt literal found');
  const judge = fpSrc.slice(judgeAt, judgeAt + 4000);
  assert.ok(/CONTRAST/i.test(judge), 'contrast mentioned in judge prompt');
  assert.ok(/CONDITIONAL/.test(judge), 'CONDITIONAL semantics in judge rules');
  assert.ok(/override/i.test(judge), 'override justification rule in judge prompt');
});

test('feature-pipeline: inline contrast helper stays drift-guarded by canonical module tests', () => {
  // The workflow script inlines a testContrast copy (runtime rejects ESM imports).
  // Drift guard: the canonical module must still exist and pass its own behavior
  // contract, and feature-pipeline must inline the same function name.
  assert.ok(/function testContrast/.test(fpSrc), 'inline testContrast in feature-pipeline.js');
  const r = testContrast({ ticketTitle: 'feat(x): y', designFiles: [], taskFiles: [], diff: '' });
  assert.equal(r.consistent, false, 'canonical module behavior contract intact');
});
