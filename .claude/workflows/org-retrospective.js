export const meta = {
  name: 'org-retrospective',
  description: 'Organization retrospective: Read agent feedback log, identify patterns, propose structural improvements',
  phases: [
    { title: 'Collect', detail: 'Read and categorize feedback entries from org-feedback.md' },
    { title: 'Analyze', detail: 'Identify recurring patterns and propose structural changes' },
    { title: 'Propose', detail: 'Present structural change proposals to CEO' },
  ],
};

// ── Phase 1: Collect ──
phase('Collect');
const feedbackAnalysis = await agent(
  `You are the Organization Analyst. Read the agent feedback log and categorize all entries.

Read: .claude/org-feedback.md

For each entry, extract:
1. Date, agent name, category
2. The core issue or suggestion
3. Whether this is a recurring theme (appears 2+ times)

Group entries by category (workflow, tooling, role-ambiguity, bottleneck, suggestion).
Count entries per category and per agent.

Also check:
- Are there any entries that contradict each other?
- Are there entries that reference the same underlying problem from different angles?
- Which categories have the most entries? (indicates systemic issues)

Return the categorized analysis.`,
  { label: 'retro:collect', phase: 'Collect', model: 'sonnet', schema: {
    type: 'object',
    properties: {
      total_entries: { type: 'number' },
      by_category: {
        type: 'object',
        properties: {
          workflow: { type: 'number' },
          tooling: { type: 'number' },
          role_ambiguity: { type: 'number' },
          bottleneck: { type: 'number' },
          suggestion: { type: 'number' },
        },
      },
      recurring_themes: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            theme: { type: 'string' },
            count: { type: 'number' },
            entries: { type: 'array', items: { type: 'string' } },
          },
        },
      },
    },
    required: ['total_entries', 'by_category', 'recurring_themes'],
  }}
);
log(`Collected ${feedbackAnalysis.total_entries} feedback entries`);

if (feedbackAnalysis.total_entries === 0) {
  return { status: 'no-feedback', message: 'No feedback entries found. Pipeline is running smoothly.' };
}

// ── Phase 2: Analyze ──
phase('Analyze');
const analysis = await agent(
  `You are the Organization Architect. Based on feedback analysis, propose concrete structural changes.

Feedback Analysis:
${JSON.stringify(feedbackAnalysis, null, 2)}

Current Organization:
Read .claude/CLAUDE.md for the current org structure and workflow descriptions.
Read .claude/agents/org/coordinator.md, .claude/agents/org/pm.md, .claude/agents/org/cto.md for role definitions.
Read .claude/workflows/feature-pipeline.js to understand the current pipeline.

For each recurring theme:
1. What is the root cause?
2. What structural change would address it?
3. What files need to change?
4. What is the risk of the change?

Rules:
- Propose changes only if they address 2+ feedback entries (avoid over-indexing on one-off issues)
- Prefer small, incremental changes over large restructurings
- Consider the "sudden mutation" principle: sometimes bold changes are better than timid tweaks

Types of changes you can propose:
- Add/remove/redefine agent roles
- Reorder pipeline phases
- Add new pipeline phases
- Change model routing (move tasks between haiku/sonnet/opus)
- Modify agent prompts or responsibilities
- Add new rules or templates
- Merge overlapping roles
- Split overloaded roles
- Change the feedback mechanism itself

For each proposal, rate:
- risk: low/medium/high (likelihood of breaking something)
- effort: trivial/small/medium/large (implementation cost)
- impact: low/medium/high (expected improvement)

Provide at most 5 proposals, ranked by impact/effort ratio.`,
  { label: 'retro:analyze', phase: 'Analyze', model: 'opus', schema: {
    type: 'object',
    properties: {
      proposals: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            title: { type: 'string' },
            rationale: { type: 'string' },
            affected_files: { type: 'array', items: { type: 'string' } },
            risk: { type: 'string', enum: ['low', 'medium', 'high'] },
            effort: { type: 'string', enum: ['trivial', 'small', 'medium', 'large'] },
            impact: { type: 'string', enum: ['low', 'medium', 'high'] },
          },
          required: ['title', 'rationale', 'risk', 'effort', 'impact'],
        },
      },
      meta_observations: {
        type: 'string',
        description: 'Observations about the feedback mechanism itself (are agents providing useful feedback? is the format working?)',
      },
    },
    required: ['proposals'],
  }}
);
log(`Generated ${analysis.proposals.length} structural change proposals`);

// ── Phase 3: Propose ──
phase('Propose');

// Log proposals for CEO review
for (const p of analysis.proposals) {
  log(`[${p.impact} impact / ${p.effort} effort / ${p.risk} risk] ${p.title}`);
}

return {
  status: 'retrospective-complete',
  feedback_summary: feedbackAnalysis,
  proposals: analysis.proposals,
  meta_observations: analysis.meta_observations,
};
