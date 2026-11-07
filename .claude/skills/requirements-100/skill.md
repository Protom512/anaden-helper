# Requirements Excellence Skill

## 100点要件定義書の基準

### 完全な要件定義書の構造

```text
1. FRONT MATTER
   ├── Document Information (version, status, classification)
   ├── Change Log (version history with approvers)
   ├── Table of Contents (with page numbers)
   ├── Executive Summary (1-page overview)
   ├── Stakeholder Register (with RACI matrix)
   └── Approval Signatures (with date and role)

2. INTRODUCTION
   ├── Purpose (why this document exists)
   ├── Scope (In-Scope/Out-of-Scope with rationale)
   ├── Context (system context diagram)
   ├── Assumptions (documented premises)
   ├── Constraints (technical, business, regulatory)
   ├── Success Definition (DoD - Definition of Done)
   └── Success Criteria (quantitative SMART goals)

3. STAKEHOLDER ANALYSIS
   ├── Stakeholder Identification
   ├── Stakeholder Expectations
   ├── RACI Matrix (Responsible, Accountable, Consulted, Informed)
   └── Communication Requirements

4. REQUIREMENTS
   ├── Functional Requirements (EARS format with IDs)
   │   ├── User Stories (As a... I want... So that...)
   │   ├── Acceptance Criteria (Given/When/Then format)
   │   ├── Priority (MoSCoW with business justification)
   │   ├── Dependency (prerequisite requirements)
   │   ├── Verification Method (Test/Review/Analysis/Demo with details)
   │   └── Traceability (Design/Test trace)
   │
   └── Non-Functional Requirements
       ├── Performance (with specific metrics)
       ├── Reliability (MTBF, availability)
       ├── Security (specific threats and mitigations)
       ├── Usability (SUS score, task completion time)
       ├── Maintainability (cyclomatic complexity limits)
       └── Scalability (concurrent users, data volume)

5. REQUIREMENTS DEPENDENCY MATRIX
   ├── Requirement-to-Requirement dependencies
   ├── External dependencies
   └── Critical path analysis

6. STATE MACHINE
   ├── Parser States (defined)
   ├── State Transitions (with conditions)
   └── State Diagram (ASCII art or Mermaid)

7. DATA FLOW
   ├── Input Specification (format, validation)
   ├── Processing Steps (with transformations)
   ├── Output Specification (format, validation)
   └── Data Flow Diagram (ASCII art or Mermaid)

8. SCENARIOS
   ├── Happy Path Scenarios (step-by-step)
   ├── Edge Case Scenarios (boundary values)
   ├── Error Scenarios (with recovery)
   └── Performance Scenarios (load patterns)

9. QUALITY ATTRIBUTES
   ├── Quality Goals (quantified)
   ├── Quality Metrics (measurement method)
   ├── Quality Thresholds (pass/fail criteria)
   └── Quality Risks (with mitigation)

10. RISK MANAGEMENT
    ├── Technical Risks (with probability, impact, mitigation)
    ├── Business Risks (with probability, impact, mitigation)
    ├── Schedule Risks (with probability, impact, mitigation)
    └── Risk Owner Assignment

11. GLOSSARY
    ├── Business Terms (domain-specific)
    ├── Technical Terms (system-specific)
    ├── Acronyms (with expansions)
    └──Translations (EN/JA bilingual)

12. APPENDICES
    ├── Requirement Traceability Matrix
    ├── Test Case Mapping
    ├── Priority Analysis (impact vs effort)
    └── References (with URLs)

13. APPROVAL
    ├── Review History (with comments)
    ├── Change Requests (with disposition)
    ├── Approval Signatures (with date, role, comments)
    └── Distribution List
```

### EARS形式完全遵守

```text
Pattern 1: Event-Driven
  When [specific event], the [system] shall [specific response]

Pattern 2: State-Driven
  While [condition exists], the [system] shall [specific response]

Pattern 3: Unwanted Behavior
  If [undesirable situation], the [system] shall [corrective action]

Pattern 4: Optional Feature
  Where [feature is present], the [system] shall [specific response]

Pattern 5: Ubiquitous
  The [system] shall [inherent property]
```

### 受入基準の完全な形式

```text
AC-[ID]: [Short Title]
  Given: [precondition]
  When: [trigger/action]
  Then: [expected outcome]
  And: [additional outcomes]

  Priority: [Must/Should/Could] - [Business Justification]
  Dependencies: [REQ-XXX, REQ-YYY]
  Verification: [Test case reference]
  Measurement: [specific metric if applicable]
```

### 100点チェックリスト

- [ ] 全ての要件に一意のIDがある (REQ-XXX)
- [ ] 全ての要件にユーザーストーリーがある
- [ ] 全ての要件に受入基準がある
- [ ] 全ての受入基準がGiven/When/Then形式
- [ ] 全ての要件に優先順位がある
- [ ] 優先順位にビジネス正当性がある
- [ ] 全ての要件に検証方法がある
- [ ] 検証方法が具体的（テストケース名、手順）
- [ ] 要件間の依存関係が明記されている
- [ ] 非機能要件が完全に定義されている
- [ ] 品質目標が定量化されている
- [ ] 品質目標に測定方法がある
- [ ] 品質目標に合格/不合格基準がある
- [ ] ステークホルダーが識別されている
- [ ] RACIマトリクスがある
- [ ] 状態遷移図がある
- [ ] データフロー図がある
- [ ] シナリオがある（正常系、異常系、境界値）
- [ ] リスク分析がある（確率、影響、緩和策）
- [ ] 用語集がある（英日対訳）
- [ ] 要件トレーサビリティマトリクスがある
- [ ] 承認署名欄がある
- [ ] 変更履歴がある（承認者付き）
- [ ] 参照文献がある（URL付き）
