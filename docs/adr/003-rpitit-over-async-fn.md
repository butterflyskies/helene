# ADR-003: RPITIT Over `async fn` in Traits

## Status

Accepted

## Context

Rust stabilized `async fn` in traits in 1.75, but bare `async fn` in a trait does not
guarantee the returned future is `Send`. This matters for any trait whose implementors
will be used across `.await` points in a multi-threaded runtime (tokio).

Two patterns were considered:

1. **`async fn` in traits** — cleaner syntax, but the returned future is not guaranteed
   `Send`. Adding a `Send` bound requires the `#[trait_variant::make(SendTrait)]`
   proc-macro or manual desugaring. The future is also not nameable, which blocks
   future `dyn` compatibility.

2. **RPITIT (return-position `impl Trait` in traits)** — explicitly returns
   `impl Future<Output = T> + Send`. The `Send` bound is visible in the trait
   definition. The future is position-based, which is compatible with the in-progress
   `dyn*` / `async fn in dyn Trait` work.

Helene's `MessageTransport` and `InferenceProvider` traits are designed to be
implemented by multiple backends (mock, HTTP, WebSocket; Anthropic, OpenAI, etc.)
and will be used as trait objects or in generic bounds across async tasks.

## Decision

Use RPITIT for all async trait methods:

```rust
fn connect(&mut self) -> impl Future<Output = Result<ConnectionId, TransportError>> + Send;
```

rather than:

```rust
async fn connect(&mut self) -> Result<ConnectionId, TransportError>;
```

## Consequences

- `Send` bounds are explicit and compiler-enforced — no runtime surprises.
- Implementors can use `async move { ... }` blocks in method bodies; the ergonomic
  cost is minimal.
- Forward-compatible with future `dyn Trait` support for async methods.
- Slightly more verbose trait definitions, but the intent is clearer.
- Applied retroactively to `MessageTransport` in PR #8; `InferenceProvider` was
  written with RPITIT from the start (PR #4).
