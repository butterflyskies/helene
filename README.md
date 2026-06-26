# helene

Provider-agnostic inference harness. Co-orbital with [Dione](https://github.com/butterflyskies/dione).

Named for Saturn's moon Helene, which shares Dione's orbit at the L4 Lagrange point. Helene sits alongside Dione — same operational space, different concern.

## What it does

Helene is the MCP client that connects to Dione and routes inference requests to LLM providers. It owns the trust boundary between the transport layer and the model context.

```
Discord → Dione (MCP Server, Streamable HTTP)
              ↕ SSE + POST
         Helene (MCP Client)
              ↕
         LLM API (Anthropic / OpenAI / DeepSeek / Gemini)
```

Three trait boundaries, one implementation each to start:

- **`MessageVerifier`** — signs and validates messages at the transport boundary. Phantoms die at `verify()`.
- **`MessageTransport`** — bidirectional message delivery between Dione and Helene.
- **`InferenceProvider`** — routes inference requests to an LLM provider.

## Status

`MessageVerifier` trait + HMAC-SHA256 implementation with proptests. Transport and provider traits are in progress.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option.
