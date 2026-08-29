export const meta = {
  name: 'release-pipeline',
  description: 'Release pipeline: Pre-release check → QC review → Release gate → Commit → Post-release verify',
  phases: [
    { title: 'Pre-check', detail: 'Verify all tests pass and code is clean' },
    { title: 'Release Review', detail: 'QC Manager runs final review' },
    { title: 'Release Gate', detail: 'CTO approves release' },
    { title: 'Commit', detail: 'Release Manager commits and tags' },
    { title: 'Verify', detail: 'Post-release verification' },
  ],
};

const branchName = typeof args === 'string' ? args : 'current-branch';
const skipReview = typeof args === 'object' && args?.skipReview === true;

// Phase 1: Pre-release check
phase('Pre-check');
const precheck = await agent(
  `Run pre-release checks on the current branch.

Execute these commands and report results:
1. git status (check for uncommitted changes)
2. git log --oneline -10 (recent commits)
3. cargo fmt --all --check (formatting)
4. cargo check --all (compilation)
5. cargo clippy --all-targets -- -D warnings (lint)
6. cargo nextest run --workspace (tests)

Report PASS/FAIL for each check.`,
  { label: 'release:precheck', phase: 'Pre-check', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      allPassed: { type: 'boolean' },
      format: { type: 'string', enum: ['PASS', 'FAIL'] },
      compilation: { type: 'string', enum: ['PASS', 'FAIL'] },
      lint: { type: 'string', enum: ['PASS', 'FAIL'] },
      tests: { type: 'string', enum: ['PASS', 'FAIL'] },
      testCount: { type: 'number' },
      uncommittedChanges: { type: 'boolean' },
    },
    required: ['allPassed', 'format', 'compilation', 'lint', 'tests'],
  }}
);
log(`Pre-check: ${precheck.allPassed ? 'ALL PASSED' : 'FAILED'}`);

if (!precheck.allPassed) {
  return { status: 'precheck-failed', precheck };
}

// Phase 2: Release Review (unless skipped)
if (!skipReview) {
  phase('Release Review');
  const releaseReview = await agent(
    `You are the QC Manager conducting a RELEASE REVIEW.

This is the final quality gate before release. Be thorough.

Changed files: Run "git diff --name-only origin/master...HEAD" to see all changes in this branch.

For each changed file:
1. Verify it matches the intended scope of the branch
2. Check for unintended side effects
3. Verify no debug/test-only code is included

Also check:
- All new public APIs are documented
- No TODO/HACK/FIXME comments left
- No test files with .only/.skip
- Version numbers are correct if changed

Decision: GO or NO-GO for release.`,
    { label: 'qc:release-review', phase: 'Release Review', model: 'sonnet', schema: {
      type: 'object',
      properties: {
        decision: { type: 'string', enum: ['GO', 'NO-GO'] },
        changedFiles: { type: 'array', items: { type: 'string' } },
        findings: { type: 'array', items: { type: 'object', properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          description: { type: 'string' },
          file: { type: 'string' },
        }}},
        releaseNotes: { type: 'string' },
      },
      required: ['decision', 'changedFiles', 'findings'],
    }}
  );
  log(`Release Review: ${releaseReview.decision}`);

  if (releaseReview.decision === 'NO-GO') {
    return { status: 'release-review-failed', precheck, releaseReview };
  }

  // Phase 3: CTO Release Gate
  phase('Release Gate');
  const gateApproval = await agent(
    `You are the CTO. This is the RELEASE GATE decision.

Pre-check: ALL PASSED
Release Review: GO
Changed files: ${JSON.stringify(releaseReview.changedFiles)}
Findings: ${JSON.stringify(releaseReview.findings)}
Release Notes: ${releaseReview.releaseNotes}

Based on the release review findings:
1. Are all critical/high findings resolved?
2. Is the scope correct for this release?
3. Any last-minute concerns?

Decision: APPROVE or REJECT release.`,
    { label: 'cto:release-gate', phase: 'Release Gate', model: 'opus', schema: {
      type: 'object',
      properties: {
        decision: { type: 'string', enum: ['APPROVE', 'REJECT'] },
        rationale: { type: 'string' },
        conditions: { type: 'array', items: { type: 'string' } },
      },
      required: ['decision', 'rationale'],
    }}
  );
  log(`CTO Gate: ${gateApproval.decision}`);

  if (gateApproval.decision === 'REJECT') {
    return { status: 'gate-rejected', precheck, releaseReview, gateApproval };
  }
}

// Phase 4: Commit
phase('Commit');
const commitResult = await agent(
  `You are the Release Manager. Execute the release commit.

Steps:
1. git add -A
2. Create commit with proper message:
   - Type: feat/fix/refactor based on branch name
   - Scope: affected crate(s)
   - Description: concise summary
   - Include: Co-Authored-By: glm 4.7 <noreply@zhipuai.cn>

3. If this is a feature branch:
   - Push the branch: git push origin HEAD
   - Create PR if needed: gh pr create --title "..." --base master

Execute the commit now.`,
  { label: 'release:commit', phase: 'Commit', model: 'sonnet' }
);
log('Commit executed');

// Phase 5: Post-release verify
phase('Verify');
const verify = await agent(
  `Post-release verification.

Run these checks to confirm the release is clean:
1. git log --oneline -3 (verify commit)
2. git status (verify clean state)
3. cargo nextest run --workspace (verify tests still pass)

Report verification result.`,
  { label: 'release:verify', phase: 'Verify', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      verified: { type: 'boolean' },
      commitHash: { type: 'string' },
      testsPassed: { type: 'boolean' },
    },
    required: ['verified', 'testsPassed'],
  }}
);

log(`Release verified: ${verify.verified ? 'SUCCESS' : 'FAILED'}`);

return {
  status: verify.verified ? 'released' : 'verify-failed',
  precheck,
  commitResult,
  verify,
};
