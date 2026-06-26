# ADR-0000: Use Architecture Decision Records

## Status

Accepted

## Context

We need to record the architectural decisions made on this project so that future
contributors (human and AI) can understand the reasoning behind the current design
without having to re-derive it from the code.

## Decision

We will use Architecture Decision Records (ADRs) as described by Michael Nygard in
his article "Documenting Architecture Decisions". Each ADR is a short markdown file
in `docs/adr/` numbered sequentially.

## Consequences

- Decisions are discoverable and durable — they survive team turnover and context switches.
- Each ADR is immutable once accepted; superseding decisions get a new ADR that references the old one.
- Low ceremony: a few paragraphs per decision, not a formal document.
