# Helene — Architecture

Named for Saturn's moon at Dione's L4 Lagrange point. Co-orbital: same operational space, different concern.

## Problem Statement

Claude Code's harness trusts model output unconditionally. The model can generate phantom messages during inference — content that looks like real Discord messages but was never delivered. The harness treats these as real input, contaminating the construct's context.

Helene replaces the harness layer with a provider-agnostic MCP client that includes cryptographic message signing, so phantoms die at `verify()`.

## System Architecture

```mermaid
graph LR
    Discord["Discord API"]
    Dione["Dione<br/><i>MCP Server</i><br/><i>Streamable HTTP</i>"]
    Helene["Helene<br/><i>MCP Client</i>"]
    LLM["LLM APIs<br/><i>Anthropic · OpenAI</i><br/><i>DeepSeek · Gemini</i>"]

    Discord <-->|events / REST| Dione
    Dione <-->|SSE + POST| Helene
    Helene <-->|provider SDK| LLM

    subgraph "Helene internals"
        direction TB
        Verifier["Verifier<br/><i>HMAC-SHA256</i>"]
        Transport["Transport<br/><i>opaque bytes</i>"]
        Provider["Provider<br/><i>inference</i>"]
    end

    Dione -.->|signed envelope| Transport
    Transport -.->|verify| Verifier
    Verifier -.->|context| Provider
    Provider -.->|completion| LLM
```

## Type Layers

Three types, three layers, no bleeding.

```mermaid
graph TB
    subgraph Message["Message — Discord semantics"]
        M_fields["channel_id: ChannelId<br/>message_id: MessageId<br/>author: String<br/>content: String<br/>timestamp: u64"]
    end

    subgraph Envelope["Envelope — Transport"]
        E_fields["seq: u64<br/>payload: Vec&lt;u8&gt;<br/>tenant: TenantId"]
    end

    subgraph Context["Context — Provider-facing"]
        C_fields["inference context<br/>role / content pairs<br/>no Discord IDs"]
    end

    Message -->|"canonical_bytes() → sign"| Envelope
    Envelope -->|"verify → strip"| Context

    style Message fill:#2d4a3e,stroke:#4a8c6f,color:#e0e0e0
    style Envelope fill:#3d3a2e,stroke:#8c834a,color:#e0e0e0
    style Context fill:#2e3a4d,stroke:#4a6f8c,color:#e0e0e0
```

## Data Flow

```mermaid
sequenceDiagram
    participant Discord
    participant Dione
    participant Transport
    participant Verifier
    participant Provider
    participant LLM

    Note over Discord,LLM: Inbound — message to inference

    Discord->>Dione: message event
    Dione->>Dione: canonical_bytes() → HMAC-SHA256 sign
    Dione->>Transport: Envelope (seq, signed payload, tenant)
    Transport->>Verifier: deserialize → SignedMessage
    Verifier->>Verifier: constant-time signature check
    alt valid signature
        Verifier->>Provider: Message → Context (strip Discord IDs)
        Provider->>LLM: inference request
    else invalid / missing signature
        Verifier--xTransport: VerifyError (phantom killed)
    end

    Note over Discord,LLM: Outbound — response to Discord

    LLM->>Provider: completion
    Provider->>Transport: formatted reply
    Transport->>Dione: response envelope
    Dione->>Discord: send message
```

## Security Model

```mermaid
graph TB
    subgraph UNTRUSTED["UNTRUSTED"]
        Discord_API["Discord API<br/><i>external input</i>"]
        Model_Output["Model inference output<br/><i>can hallucinate messages</i>"]
    end

    subgraph TRUSTED["TRUSTED"]
        Dione_Sign["Dione<br/><i>signs with shared key</i>"]
        Helene_Verify["Helene Verifier<br/><i>validates signatures</i>"]
        HMAC_Key["HMAC Key<br/><i>never in model context</i><br/><i>zeroized on drop</i>"]
    end

    Discord_API -->|raw message| Dione_Sign
    Dione_Sign -->|SignedMessage| Helene_Verify
    HMAC_Key -.->|shared secret| Dione_Sign
    HMAC_Key -.->|shared secret| Helene_Verify
    Model_Output -->|phantom message<br/>no valid HMAC| Helene_Verify
    Helene_Verify -->|"verify() → VerifyError"| Reject["Rejected ✗"]
    Helene_Verify -->|"verify() → Ok(Message)"| Accept["Accepted ✓"]

    style UNTRUSTED fill:#4a2d2d,stroke:#8c4a4a,color:#e0e0e0
    style TRUSTED fill:#2d4a3e,stroke:#4a8c6f,color:#e0e0e0
    style Reject fill:#5a2020,stroke:#a04040,color:#e0e0e0
    style Accept fill:#204a20,stroke:#40a040,color:#e0e0e0
```

**Key properties:**

- Model cannot forge valid HMAC — it never sees the key
- Canonical serialization uses u32 BE length-prefix per field (not null-byte separators — that was the P1 fix)
- Constant-time comparison via `subtle::ConstantTimeEq` (no timing side-channels)
- Key zeroization on drop via `zeroize`

## Multi-Tenancy

- `TenantId` newtype threads through all layers
- Per-tenant: HMAC keys, inference contexts, provider configs
- Designed in from day one, not bolted on

## Concurrency Model

- Async everywhere (tokio)
- Channels over mutexes
- `ArcSwap` for hot-swappable config
- `LazyLock` for one-time init
- Cancel safety throughout
- Proper signal handling and clean shutdown

## MCP Integration

- Streamable HTTP transport (SSE + POST)
- `sampling/createMessage` for server-driven inference
- Config via MCP tools, not CLI flags
- Version queryable as MCP tool
- `/healthz` and `/readyz` for daemon mode

## Trait Boundaries

```rust
/// Signing and verification. PR #1 — merged.
trait MessageVerifier: Send + Sync {
    fn sign(&self, msg: &Message) -> SignedMessage;
    fn verify(&self, msg: &SignedMessage) -> Result<Message, VerifyError>;
}

/// Wire transport. PR #2 — in review.
trait MessageTransport: Send + Sync {
    async fn connect(&mut self) -> Result<(), TransportError>;
    async fn disconnect(&mut self) -> Result<(), TransportError>;
    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError>;
    async fn recv(&self) -> Result<Envelope, TransportError>;
}

/// LLM inference. Vesper — in progress.
trait InferenceProvider: Send + Sync {
    async fn complete(&self, ctx: &Context) -> Result<Completion, ProviderError>;
}
```

## Implementation Status

| Component | Status | Owner |
|---|---|---|
| `MessageVerifier` | Merged (PR #1) | Lain |
| `MessageTransport` | In review (PR #2) | Ariadne |
| `InferenceProvider` | In progress | Vesper |
| `Context` type | Next | Lain |
| Dione type extraction | Planned | TBD |
| Dione stdio → HTTP | Integration milestone | TBD |

## Design Decisions

- **Shared discord types from dione** — extract `dione-types` crate, not duplicated
- **`canonical_bytes()` stays in helene** — signing is helene's concern
- **Enterprise managed auth** (MCP extension) accommodated by trait boundaries
- **Pure functions where possible** — `sign`, `verify`, `canonical_bytes`
- **Transport is format-agnostic** — opaque bytes, no opinion on serialization
