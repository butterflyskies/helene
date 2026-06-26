# ADR-006: Dione as Shared Types Source

## Status

Accepted

## Context

Helene and Dione share Discord-related types (`ChannelId`, `MessageId`, etc.). These
types need to be consistent across both crates. Three approaches were considered:

1. **Duplicate types in each crate** — simple but creates drift risk. Changes to the
   type in one crate may not propagate to the other.

2. **Extract a `dione-types` crate** — clean separation, but introduces a third crate
   to version, publish, and coordinate releases for. During 0.x rapid iteration, the
   coordination overhead outweighs the benefit.

3. **Use dione as a library dependency** — helene depends on dione and imports its
   types directly. No duplication, no third crate. The cost is a heavier dependency
   (dione carries its MCP server code), but this can be gated behind a feature flag
   or refactored later.

## Decision

Use dione as a library for shared types rather than extracting a separate crate.
Helene imports Discord types from dione's public API.

This is explicitly a "for now" decision — when the type surface stabilizes and the
dependency weight matters, extract a `dione-types` (or `helene-types`) crate.

## Consequences

- Zero duplication — single source of truth for shared types.
- No third crate to version and release during rapid 0.x iteration.
- Helene takes a dependency on the full dione crate (mitigated by feature flags if
  needed).
- Refactoring to a separate types crate later is straightforward — the import paths
  change but the types don't.
- Must ensure dione's public type API is stable enough for helene to depend on.
