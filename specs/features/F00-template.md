<!--
  SPEC TEMPLATE — copy this to features/F##-name.md or foundations/E##-name.md.
  Fill the <placeholders>, delete sections you don't need, and delete these
  HTML comments as you go. Before writing any body text, read references/writing-style.md.
  Always keep Purpose, Detailed Specification, Cross-References, and Changelog.
-->

# F## — <Feature Name>

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** <YYYY-MM-DD>
>
> **Purpose:** <One or two sentences. What this spec defines and what it delivers. If you can't say it in two sentences, the spec is doing too much.>
>
> **Depends on:** [<spec>](<path>.md)   ·   **Related:** [<spec>](<path>.md)

> Requirement tag: **<TAG>**   <!-- short uppercase prefix for REQ-<TAG>-NN; delete this line if the spec won't use requirement IDs -->

---

## 1. Purpose & Scope

<!-- Open with the plain-language summary, then list what's in scope. -->

<One-sentence plain-language summary of what this feature is.>

This spec covers:

- <in-scope item>
- <in-scope item>

## 2. Non-Goals / Out of Scope

<!-- What this deliberately excludes, and who owns it instead. -->

- <thing this spec does NOT cover> — owned by [<spec>](<path>.md).

## 3. Background & Rationale

<!-- Why this exists and how it fits the whole. Keep it short. -->

<Context the reader needs to follow the decisions below.>

## 4. Concepts & Definitions

<!-- Terms used or introduced. Canonical terms → link to glossary.md instead of redefining. -->

- **<Term>** — <definition>. (Canonical definition in [glossary](../glossary.md).)

## 5. Detailed Specification

<!-- The body. Use numbered subsections. Lead each with a plain summary.
     If using requirement IDs, give each load-bearing rule a REQ-<TAG>-NN. -->

### 5.1 <Subsection — e.g. Routes>

<Plain-language summary of this subsection.>

**REQ-<TAG>-01 — <short title of the rule>.**

<The rule, one idea per sentence. State pre/postconditions where they matter.>

### 5.2 Data model

<Say what these tables are before showing them. Label code with its file path.>

```dart
// lib/data/tables/<name>.dart
class <Name>s extends Table {
  IntColumn get id => integer().autoIncrement()();
  // ...
}
```

<Then explain the keys, indexes, and nullability rules in prose.>

### 5.3 Screens

<Describe each surface. Include an ASCII mockup (~78 cols) and its states.>

```
┌──────────────────────────────────────┐
│  <Screen title>                       │
│                                       │
│  <layout sketch>                      │
└──────────────────────────────────────┘
```

States: empty · loading · error · <feature-specific>.

## 6. Visualizations

<!-- Mermaid for flows/lifecycles, ASCII for screens, tables for matrices.
     Follow references/mermaid.md — init block, labeled arrows, colored nodes. -->

```mermaid
%%{init: {'theme': 'base', 'themeVariables': {'fontSize': '14px'}}}%%
stateDiagram-v2
    [*] --> Idle
    Idle --> Active: start
    Active --> [*]: finish
```

## 7. Data Shapes

<!-- Concrete payloads crossing a boundary. Quote verbatim — it's a contract. -->

```json
{ "id": "string", "createdAt": "ISO-8601" }
```

## 8. Examples & Use Cases

<!-- Walk a realistic scenario using the constitution's example cast. -->

<A concrete walk-through with the recurring characters/data.>

## 9. Edge Cases & Failure Modes

<!-- Empty states, failures, races, conflicting input. Not just the happy path. -->

- <case> → <behavior>.

## 10. Open Questions & Decisions

<!-- Undecided items as OQ-<TAG>-NN; record resolved decisions too. -->

- **OQ-<TAG>-1** — <open question>.

## 11. Cross-References

<!-- The complete list of connected specs, grouped like the header. -->

- **Depends on:** [<spec>](<path>.md) — <why>.
- **Related:** [<spec>](<path>.md) — <why>.

## 12. Changelog

<!-- Newest first. ISO dates. Narrative entries — what changed and why. -->

- **<YYYY-MM-DD>** — Initial draft.
