
export const meta = {
  name: 'comprehensive-commit-gate',
  description: 'Full-spectrum commit gate: functional + reliability + performance + extensibility + governance + security, with consensus on judgment calls',
  phases: [
    { title: 'Non-Functional Review', detail: '6 independent reviewers evaluate across dimensions' },
    { title: 'Consensus', detail: 'Adversarial consensus on GO/NO-GO with documented reasoning' },
  ],
}

const V_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['GO', 'NO-GO', 'CONDITIONAL'] },
    dimension: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          severity: { type: 'string', enum: ['critical', 'major', 'minor', 'info', 'praise'] },
          title: { type: 'string' },
          detail: { type: 'string' },
          evidence: { type: 'string' },
          suggestion: { type: 'string' },
        },
        required: ['severity', 'title', 'detail']
      }
    },
    consensus_notes: { type: 'string' },
    summary: { type: 'string' },
  },
  required: ['verdict', 'dimension', 'findings', 'summary']
}

// ── Phase 1: Non-Functional Review ──
phase('Non-Functional Review')

const reviews = await parallel([

  // 1. Reliability & Error Handling — 例外処理・エラーハンドリングの完全性
  () => agent(
    `You are a RELIABILITY REVIEWER. You evaluate exception handling, error paths, and fault tolerance.

## Refactoring Context
parser.rs (5693 lines) was split into 7 submodules in crates/tsql-parser/src/parser/.
No behavior change intended — pure structural refactoring.

## Your Review Scope

### 1. Error Path Continuity
Read the dispatcher in mod.rs (parse_statement method). For EVERY match arm:
- Does the called method exist in the correct submodule?
- Is the error type returned identical to the original?
- Are there match arms that could panic (unwrap/expect) on unexpected input?

### 2. Exception Handling
- Read misc.rs, control_flow.rs, dml.rs — do they handle ALL error paths?
- Are there any new early returns that skip error recovery?
- Is the synchronize() function still called at the right points?
- After the split, can errors still propagate correctly through impl blocks in different files?

### 3. Edge Cases
- Empty input handling
- EOF during statement parsing
- Recursive depth limits (check_depth_before_nesting)
- What happens if a submodule method panics? Is it caught?

### 4. Resource Safety
- Any risk of infinite loops in parsing after the split?
- Buffer state consistency across submodule method calls?

Read the actual code. Provide specific line references for each finding.`,
    { label: 'review:reliability', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),

  // 2. Performance — 性能性 (コンパイル時間・ランタイム影響)
  () => agent(
    `You are a PERFORMANCE REVIEWER. You evaluate performance impact of the refactoring.

## Context
parser.rs split into 7 submodules. Rust compiles each file as a separate codegen unit when possible.

## Your Review Scope

### 1. Compile Time Impact
- Run: time cargo check -p tsql-parser 2>&1
- Are there circular module dependencies that prevent parallel compilation?
- Does the split improve or worsen incremental compilation?

### 2. Runtime Performance
- Are there any new vtable dispatches or dynamic dispatch introduced? (Hint: splitting impl blocks does NOT introduce dynamic dispatch in Rust)
- Is inlining affected? Methods in different files but same crate CAN still be inlined (pub(crate) is sufficient for cross-file inlining within a crate)
- Check: are methods marked pub(super) vs pub(crate)? Does visibility affect optimization?

### 3. Binary Size
- Run: cargo bloat -p tsql-parser --release 2>&1 || echo "cargo-bloat not installed"
- Is there any code bloat from the split?

### 4. API Hot Path
- parse_statement() is the hot path. Read it in mod.rs.
- Does the dispatcher add any overhead vs the original monolith?
- Are match arms exhaustive and non-overlapping?

Provide evidence (measurements where possible).`,
    { label: 'review:performance', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),

  // 3. Extensibility & Maintainability — 拡張性・保守性
  () => agent(
    `You are an EXTENSIBILITY REVIEWER. You evaluate future-proofing and maintainability.

## Context
parser.rs split into: mod.rs, select.rs, dml.rs, ddl.rs, control_flow.rs, misc.rs, helpers.rs

## Your Review Scope

### 1. Module Cohesion
Read each submodule. Rate cohesion (HIGH/MEDIUM/LOW) for each:
- select.rs — are all SELECT-related methods truly related?
- dml.rs — INSERT/UPDATE/DELETE + variable assignment: is this cohesive?
- ddl.rs — CREATE/ALTER + data types + constraints: too many responsibilities?
- control_flow.rs — IF/WHILE/BEGIN/TRY..CATCH: cohesive?
- misc.rs — is this a dumping ground or cohesive "session/procedural" module?
- helpers.rs — are helpers truly shared or should some be module-specific?

### 2. Adding a New Statement Type
Walk through the steps needed to add a new statement (e.g., DROP TABLE):
1. Which files need modification?
2. Is the dispatcher in mod.rs easy to extend?
3. Are there hidden dependencies?

### 3. Method Visibility Design
- Are pub(super) and pub(crate) used correctly?
- Can external crates accidentally access internal parsing methods?
- Is the module hierarchy deep enough/too deep?

### 4. Naming and Discoverability
- If you were a new developer, could you find where SELECT is parsed without grep?
- Are file names self-documenting?

### 5. Judgment Call: misc.rs naming
There's no perfect name for DECLARE/SET/TRANSACTION/THROW/EXEC grouping.
Evaluate: is "misc" acceptable? Propose alternatives with pros/cons.
This is a judgment call — document your reasoning.`,
    { label: 'review:extensibility', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),

  // 4. Governance & Documentation — 統制・ドキュメント
  () => agent(
    `You are a GOVERNANCE REVIEWER. You evaluate documentation, traceability, and compliance.

## Context
Structural refactoring of parser.rs into submodules.

## Your Review Scope

### 1. Documentation Completeness
- Read each submodule's file-level doc comment (//! or ///)
- Are module purposes documented?
- Are public methods documented? (check pub(super) methods)
- Is the original parser.rs module-level doc preserved in mod.rs?
- Are there any doc comments that became stale after the move?

### 2. Commit Traceability
- This will be a single commit "refactor(parser): split parser.rs into submodules"
- Is this sufficient? Or should it be multiple commits?
- What should the commit body contain?

### 3. CHANGELOG Impact
- Should this refactoring appear in a CHANGELOG?
- Is it a breaking change for downstream consumers?
- Check crates/tsql-parser/src/lib.rs — are re-exports unchanged?

### 4. Architecture Documentation
- Does this split require updating any documentation files?
- Check CLAUDE.md or MEMORY.md — do they reference the old structure?

### 5. Review Audit Trail
- For governance: document that this refactoring was reviewed by multiple perspectives
- What test evidence exists to prove behavior preservation?

Provide specific file references for documentation gaps.`,
    { label: 'review:governance', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),

  // 5. Security & Safety — セキュリティ・安全性
  () => agent(
    `You are a SECURITY REVIEWER. You evaluate safety, resource management, and attack surface.

## Context
T-SQL parser split into submodules. This is a library crate used by a Language Server.

## Your Review Scope

### 1. Memory Safety
- Grep for "unsafe" in all parser/*.rs files
- Any raw pointer usage after the split?
- Any transmute, from_raw, or other unsafe operations?

### 2. Denial of Service Vectors
- Recursive parsing: is DEFAULT_MAX_DEPTH still enforced?
- Read control_flow.rs — does parse_block check depth before recursion?
- Can malformed input cause stack overflow through the new module structure?
- Is the error recovery (synchronize) still reachable from all parse paths?

### 3. Input Validation
- Does the split change how untrusted SQL input is handled?
- Are there any new code paths that skip validation?
- Is the TokenBuffer bounds-checked in all submodule accesses?

### 4. Panic Safety
- Grep for unwrap, expect, panic in ALL parser/*.rs files (not in #[cfg(test)])
- Are there any new panics introduced by the split?
- Is the library code panic-free as per project rules?

### 5. Dependency Safety
- Does the split introduce any new dependencies?
- Are all imports from trusted crate-internal modules?

Run: grep -rn "unsafe\|unwrap()\|expect(\|panic!" crates/tsql-parser/src/parser/*.rs
Report exact findings.`,
    { label: 'review:security', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),

  // 6. Integration & Connectivity — 接続性・統合性
  () => agent(
    `You are an INTEGRATION REVIEWER. You evaluate how this change affects the wider system.

## Context
tsql-parser is a library used by:
- ase-ls-core (Language Server core)
- mysql-emitter (SQL transpiler)
- ase-ls (Language Server binary)

## Your Review Scope

### 1. Downstream Compilation
Run: cargo check --all 2>&1
- Do all downstream crates compile?
- Are there any new warnings in downstream crates?

### 2. Public API Stability
Read crates/tsql-parser/src/lib.rs
- Are ALL re-exports identical to before the split?
- Is Parser still publicly accessible?
- Are ParserMode, ParseError, etc. still re-exported?
- Is there any change to the public API surface?

### 3. Integration Test Coverage
Run: cargo nextest run --workspace 2>&1
- Total test count: should be 1175 passed
- Are there any integration tests in ase-ls or ase-ls-core that exercise parser directly?
- Run: grep -rn "Parser::new\|parse(" crates/ase-ls-core/tests/ crates/ase-ls/tests/ 2>/dev/null
- Do these still pass?

### 4. Cross-Module Dependencies
After the split, is there any submodule that imports from another submodule?
Run: grep -rn "use super::" crates/tsql-parser/src/parser/
Run: grep -rn "use crate::parser::" crates/tsql-parser/src/parser/
Cross-submodule imports create coupling — document them.

### 5. Dogfood Tests
Check crates/tsql-parser/tests/ — do dogfood tests still work?
Check crates/ase-ls/tests/ — any parser-related tests?

Provide evidence for each point.`,
    { label: 'review:integration', phase: 'Non-Functional Review', schema: V_SCHEMA }
  ),
])

// ── Phase 2: Consensus ──
phase('Consensus')

const consensus = await agent(
  `You are the CONSENSUS JUDGE. You synthesize 6 independent reviews into a final decision.

## Reviewer Verdicts
${JSON.stringify(reviews, null, 2)}

## Your Task

### 1. Verdict Matrix
Create a table:
| Dimension | Verdict | Key Finding | Confidence |

### 2. Blocking Issues
List ONLY issues that must be fixed before committing:
- Must be CRITICAL or MAJOR severity
- Must have concrete evidence (not theoretical)
- Must be fixable (not inherent limitations)

### 3. Judgment Calls — Where No Single Answer Exists
For each judgment call raised by reviewers (e.g., misc.rs naming, commit granularity):
- Present both sides of the argument
- Document the consensus recommendation
- Note any dissenting opinions

### 4. Follow-Up Items
List issues that are real but non-blocking:
- Should be addressed in future commits
- Include suggested priority and effort

### 5. Final Verdict
GO = commit now, no blocking issues
NO-GO = fix blocking issues first
CONDITIONAL = GO if specific conditions are met within this session

### 6. Commit Specification
If GO or CONDITIONAL:
- Exact commit message (conventional commits format)
- Commit body content
- Co-author attribution

Be rigorous. A false GO is worse than a cautious NO-GO.`,
  { label: 'judge:consensus', phase: 'Consensus', schema: {
    type: 'object',
    properties: {
      verdict_matrix: { type: 'string' },
      blocking_issues: { type: 'array', items: { type: 'string' } },
      judgment_calls: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            topic: { type: 'string' },
            positions: { type: 'array', items: { type: 'string' } },
            consensus: { type: 'string' },
            dissent: { type: 'string' },
          },
          required: ['topic', 'positions', 'consensus']
        }
      },
      follow_up_items: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            item: { type: 'string' },
            priority: { type: 'string' },
            effort: { type: 'string' },
        },
          required: ['item', 'priority', 'effort']
        }
      },
      final_verdict: { type: 'string', enum: ['GO', 'NO-GO', 'CONDITIONAL'] },
      commit_message: { type: 'string' },
      commit_body: { type: 'string' },
    },
    required: ['verdict_matrix', 'blocking_issues', 'judgment_calls', 'follow_up_items', 'final_verdict', 'commit_message']
  }}
)

return consensus
