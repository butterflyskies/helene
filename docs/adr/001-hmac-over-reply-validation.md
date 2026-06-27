# ADR-001: HMAC Signing Over Reply-to API Validation

## Status

Accepted

## Context

Helene needs to distinguish real Discord messages from model-hallucinated "phantom"
messages. Two approaches were considered:

1. **Reply-to API checks** — call the Discord API to confirm each message ID actually
   exists before processing it. This requires network round-trips, depends on Discord
   API availability, and introduces rate-limit pressure.

2. **HMAC signing** — Dione signs each message with a shared secret using HMAC-SHA256
   before forwarding it. Helene verifies the signature locally. No network calls, no
   external dependencies.

The model never sees the HMAC key, so it cannot forge valid signatures. A hallucinated
message will always fail `verify()`.

## Decision

Use HMAC-SHA256 signing at the Dione/Helene boundary. Dione signs outbound messages
with a shared key; Helene verifies inbound messages with the same key. Messages without
a valid signature are rejected as phantoms.

The key is zeroized on drop (`zeroize` crate) and never appears in model context.
Signature comparison uses constant-time equality (`subtle::ConstantTimeEq`) to prevent
timing side-channels.

## Consequences

- Verification is local and zero-latency — no Discord API calls needed.
- No dependency on Discord API availability for message authentication.
- Requires secure key distribution between Dione and Helene (shared secret).
- Key compromise would allow phantom injection — but the key never enters model context,
  so the attack surface is limited to the deployment environment.
- Replay attacks are mitigated by sequence numbers on the transport envelope, not by
  the HMAC itself.
