# ADR-004: No `#[non_exhaustive]` During 0.x

## Status

Accepted

## Context

`#[non_exhaustive]` prevents downstream crates from exhaustively matching on an enum
or constructing a struct, allowing the defining crate to add variants/fields in minor
versions without a semver-breaking change.

This is valuable for published 1.x crates where API stability matters. But helene is
at 0.x — semver explicitly permits breaking changes in any 0.x release. Adding
`#[non_exhaustive]` during 0.x:

- Imposes ergonomic costs on internal consumers (wildcard matches, builder patterns)
  for a guarantee that semver already provides.
- Prevents exhaustive matching in tests, which is useful for catching unhandled
  variants during rapid iteration.
- Signals stability intent that doesn't match the crate's actual maturity.

## Decision

Strip `#[non_exhaustive]` from all types during the 0.x development phase. Revisit
when approaching 1.0 — at that point, add it to public enums and structs that are
likely to grow.

## Consequences

- Internal code (tests, sibling crates) can use exhaustive matches freely.
- Adding enum variants or struct fields is a breaking change under strict semver
  interpretation, but 0.x already permits this.
- Must remember to re-evaluate `#[non_exhaustive]` before 1.0 release.
- Downstream consumers (if any exist during 0.x) should pin to exact versions.
