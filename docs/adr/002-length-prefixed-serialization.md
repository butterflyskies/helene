# ADR-002: Length-Prefixed Canonical Serialization

## Status

Accepted

## Context

`canonical_bytes()` produces a deterministic byte representation of a `Message` for
HMAC signing. The serialization must be unambiguous — given any two distinct messages,
their canonical bytes must differ.

Two delimiter strategies were considered:

1. **Null-byte separators** (`\0` between fields) — simple, but if any field value
   contains a null byte, field boundaries become ambiguous. An attacker (or a bug)
   could craft content that shifts bytes between fields and produces the same
   serialized output as a different message. This is a canonicalization collision.

2. **Length-prefixed fields** (u32 big-endian length prefix before each field) —
   each field is preceded by its byte length. The deserializer knows exactly where
   each field ends regardless of content. No byte value is "special", so no content
   can cause ambiguity.

## Decision

Use u32 big-endian length prefixes for variable-length fields in `canonical_bytes()`.
Fixed-width numeric fields (e.g. `timestamp: u64`) are written directly as big-endian
bytes without a length prefix — their size is known statically, so no prefix is needed
for unambiguous parsing.

The format is:
`[len][channel_id][len][message_id][timestamp BE u64][len][author][len][content]`
where each `len` is a 4-byte big-endian u32.

## Consequences

- Collision-resistant by construction — no content can cause field boundary ambiguity.
- Slightly more bytes on the wire than null-byte separation (4 bytes per field overhead).
- Deterministic and platform-independent (big-endian, fixed-width lengths).
- Maximum field size is 4 GiB (u32::MAX) — sufficient for Discord messages.
- Simple to implement and audit; no escaping or quoting logic needed.
