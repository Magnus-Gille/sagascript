# Architecture decision records

Short records of decisions that shape how Sagascript is built, with the evidence behind them.
AGENTS.md states the rules in one line each; the ADRs say why, what follows from them, and
where the code shows them. The map of the code is [ARCHITECTURE.md](../ARCHITECTURE.md).

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-cli-first.md) | CLI first | Accepted |
| [0002](0002-privacy-first.md) | Privacy first: local by default, remote opt-in | Accepted |
| [0003](0003-main-thread-native-apis.md) | Native input and UI APIs run on the main thread | Accepted |
| [0004](0004-test-strategy.md) | Test strategy | Accepted |
| [0005](0005-mutex-poison-policy.md) | Mutex poison policy: recover, log, restore a safe state | Accepted |
| [0006](0006-typed-errors.md) | Typed errors across boundaries: `{ code, message, retryable }`, match on `code`, scrub paths | Accepted |
| [0007](0007-settings-file-evolution.md) | Settings file evolution: `schema_version`, ordered migrations, lenient fields | Accepted |

## Writing an ADR

1. Take the next free number; never reuse or renumber. File name:
   `NNNN-short-title.md`.
2. Copy the template below. Keep it under about 80 lines and cite files, issues or commits
   as evidence rather than restating code.
3. Add it to the index. If it changes a rule, update the one-line rule in
   [AGENTS.md](../../AGENTS.md) in the same PR.
4. To change a decision, write a new ADR and set the old one's status to
   `Superseded by NNNN`; do not rewrite history.

Status values: `Proposed`, `Accepted`, `Superseded by NNNN`, `Deprecated`.

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD
- Related: #issue, other ADRs

## Context

What forces the decision: the problem, constraints and evidence (files, issues, incidents).

## Decision

What we do, stated as rules someone can follow and check.

## Consequences

What becomes easier or harder, what to watch for, and known gaps (link the issues).
```
