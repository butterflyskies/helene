use proptest::prelude::*;

use super::*;

// --- Strategy helpers ---

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::System),
        Just(Role::User),
        Just(Role::Assistant),
        Just(Role::Tool),
    ]
}

fn arb_message() -> impl Strategy<Value = ChatMessage> {
    prop_oneof![
        (arb_role(), ".*").prop_map(|(role, content)| ChatMessage::new(role, content)),
        ("[a-z0-9_]{1,20}", ".*").prop_map(|(id, content)| ChatMessage::tool_result(id, content)),
    ]
}

fn arb_model_id() -> impl Strategy<Value = ModelId> {
    "[a-z0-9-]{1,40}".prop_map(ModelId::new)
}

fn arb_stop_reason() -> impl Strategy<Value = StopReason> {
    prop_oneof![
        Just(StopReason::EndTurn),
        Just(StopReason::MaxTokens),
        Just(StopReason::ToolUse),
        Just(StopReason::StopSequence),
    ]
}

fn arb_usage() -> impl Strategy<Value = Usage> {
    (any::<u32>(), any::<u32>()).prop_map(|(i, o)| Usage::new(i, o))
}

fn arb_tool_call() -> impl Strategy<Value = ToolCall> {
    ("[a-z0-9_]{1,20}", "[a-z_]{1,20}", "\\{.*\\}").prop_map(|(id, name, args)| ToolCall {
        id,
        name,
        arguments: args,
    })
}

fn arb_tool_definition() -> impl Strategy<Value = ToolDefinition> {
    ("[a-z_]{1,20}", ".*", "\\{.*\\}").prop_map(|(name, desc, schema)| ToolDefinition {
        name,
        description: desc,
        input_schema: schema,
    })
}

fn arb_response_content() -> impl Strategy<Value = ResponseContent> {
    prop_oneof![
        ".*".prop_map(ResponseContent::Text),
        (
            prop::collection::vec(arb_tool_call(), 1..=3),
            prop::option::of(".*")
        )
            .prop_map(|(calls, text)| ResponseContent::ToolCalls { calls, text }),
    ]
}

fn arb_completion_request() -> impl Strategy<Value = CompletionRequest> {
    (
        arb_model_id(),
        prop::collection::vec(arb_message(), 1..=5),
        1u32..=100_000u32,
        prop::option::of(0.0f64..=2.0f64),
        prop::option::of(prop::collection::vec(arb_tool_definition(), 1..=3)),
    )
        .prop_map(
            |(model, messages, max_tokens, temperature, tools)| CompletionRequest {
                model,
                messages,
                max_tokens,
                temperature,
                tools,
            },
        )
}

fn arb_completion_response() -> impl Strategy<Value = CompletionResponse> {
    (
        arb_response_content(),
        arb_model_id(),
        arb_usage(),
        arb_stop_reason(),
    )
        .prop_map(|(content, model, usage, stop_reason)| CompletionResponse {
            content,
            model,
            usage,
            stop_reason,
        })
}

// --- Construction tests ---

#[test]
fn message_constructors() {
    let sys = ChatMessage::system("you are helpful");
    assert_eq!(sys.role, Role::System);
    assert_eq!(sys.content, "you are helpful");

    let usr = ChatMessage::user("hello");
    assert_eq!(usr.role, Role::User);
    assert_eq!(usr.content, "hello");
    assert!(usr.tool_call_id.is_none());
    assert!(usr.tool_calls.is_none());

    let ast = ChatMessage::assistant("hi there");
    assert_eq!(ast.role, Role::Assistant);
    assert_eq!(ast.content, "hi there");
    assert!(ast.tool_call_id.is_none());
    assert!(ast.tool_calls.is_none());

    let tool = ChatMessage::tool(r#"{"result": 42}"#);
    assert_eq!(tool.role, Role::Tool);
    assert_eq!(tool.content, r#"{"result": 42}"#);
    assert!(tool.tool_call_id.is_none());
    assert!(tool.tool_calls.is_none());
}

#[test]
fn tool_result_constructor() {
    let msg = ChatMessage::tool_result("call_123", r#"{"result": 42}"#);
    assert_eq!(msg.role, Role::Tool);
    assert_eq!(msg.content, r#"{"result": 42}"#);
    assert_eq!(msg.tool_call_id.as_deref(), Some("call_123"));
    assert!(msg.tool_calls.is_none());
}

#[test]
fn assistant_with_tool_calls_constructor() {
    let calls = vec![ToolCall {
        id: "call_1".into(),
        name: "search".into(),
        arguments: r#"{"q":"test"}"#.into(),
    }];
    let msg = ChatMessage::assistant_with_tool_calls("thinking...", calls.clone());
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(msg.content, "thinking...");
    assert_eq!(msg.tool_calls.as_ref().unwrap(), &calls);
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn model_id_display() {
    let id = ModelId::new("claude-opus-4-20250514");
    assert_eq!(id.to_string(), "claude-opus-4-20250514");
}

#[test]
fn role_display() {
    assert_eq!(Role::System.to_string(), "system");
    assert_eq!(Role::User.to_string(), "user");
    assert_eq!(Role::Assistant.to_string(), "assistant");
    assert_eq!(Role::Tool.to_string(), "tool");
}

#[test]
fn stop_reason_display() {
    assert_eq!(StopReason::EndTurn.to_string(), "end_turn");
    assert_eq!(StopReason::MaxTokens.to_string(), "max_tokens");
    assert_eq!(StopReason::ToolUse.to_string(), "tool_use");
    assert_eq!(StopReason::StopSequence.to_string(), "stop_sequence");
}

#[test]
fn usage_display_and_total() {
    let u = Usage::new(100, 50);
    assert_eq!(u.to_string(), "100in/50out");
    assert_eq!(u.total(), 150);
}

#[test]
fn usage_total_overflow_saturates() {
    let u = Usage::new(u32::MAX, 1);
    assert_eq!(u.total(), u32::MAX);
}

#[test]
fn completion_request_construction() {
    let req = CompletionRequest {
        model: ModelId::new("test-model"),
        messages: vec![ChatMessage::user("hi")],
        max_tokens: 1024,
        temperature: Some(0.7),
        tools: None,
    };
    assert_eq!(req.model.0, "test-model");
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.max_tokens, 1024);
    assert_eq!(req.temperature, Some(0.7));
    assert!(req.tools.is_none());
}

#[test]
fn completion_response_text() {
    let resp = CompletionResponse {
        content: ResponseContent::Text("hello world".into()),
        model: ModelId::new("test-model"),
        usage: Usage::new(10, 5),
        stop_reason: StopReason::EndTurn,
    };
    assert!(matches!(resp.content, ResponseContent::Text(ref s) if s == "hello world"));
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}

#[test]
fn completion_response_tool_calls() {
    let tc = ToolCall {
        id: "call_1".into(),
        name: "get_weather".into(),
        arguments: r#"{"city":"tokyo"}"#.into(),
    };
    let resp = CompletionResponse {
        content: ResponseContent::ToolCalls {
            calls: vec![tc],
            text: None,
        },
        model: ModelId::new("test-model"),
        usage: Usage::new(20, 10),
        stop_reason: StopReason::ToolUse,
    };
    match &resp.content {
        ResponseContent::ToolCalls { calls, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "get_weather");
        }
        _ => panic!("expected tool calls"),
    }
}

#[test]
fn tool_definition_construction() {
    let td = ToolDefinition {
        name: "search".into(),
        description: "Search the web".into(),
        input_schema: r#"{"type":"object"}"#.into(),
    };
    assert_eq!(td.name, "search");
}

#[test]
fn provider_error_display() {
    let e = ProviderError::Auth("bad token".into());
    assert_eq!(e.to_string(), "authentication failed: bad token");

    let e = ProviderError::RateLimited {
        retry_after_ms: Some(5000),
    };
    assert_eq!(e.to_string(), "rate limited; retry after Some(5000)ms");

    let e = ProviderError::RateLimited {
        retry_after_ms: None,
    };
    assert_eq!(e.to_string(), "rate limited; retry after Nonems");

    let e = ProviderError::ModelNotFound(ModelId::new("gpt-9"));
    assert_eq!(e.to_string(), "model not found: gpt-9");

    let e = ProviderError::ContextLength {
        used: 200_000,
        limit: 128_000,
    };
    assert_eq!(
        e.to_string(),
        "context length exceeded: 200000 tokens exceeds 128000"
    );

    let e = ProviderError::Unavailable("server down".into());
    assert_eq!(e.to_string(), "provider unavailable: server down");

    let e = ProviderError::Request("timeout".into());
    assert_eq!(e.to_string(), "request failed: timeout");
}

// --- Proptests ---

proptest! {
    #[test]
    fn model_id_display_roundtrip(id in arb_model_id()) {
        let displayed = id.to_string();
        let reconstructed = ModelId::new(displayed.clone());
        prop_assert_eq!(reconstructed.0, displayed);
    }

    #[test]
    fn role_display_is_lowercase(role in arb_role()) {
        let s = role.to_string();
        prop_assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }

    #[test]
    fn usage_total_is_sum(inp in any::<u32>(), out in any::<u32>()) {
        let u = Usage::new(inp, out);
        prop_assert_eq!(u.total(), inp.saturating_add(out));
    }

    #[test]
    fn completion_request_preserves_message_count(req in arb_completion_request()) {
        prop_assert!(!req.messages.is_empty());
        prop_assert!(req.messages.len() <= 5);
    }

    #[test]
    fn completion_response_clone_eq(resp in arb_completion_response()) {
        let cloned = resp.clone();
        prop_assert_eq!(resp, cloned);
    }

    #[test]
    fn message_clone_preserves_fields(msg in arb_message()) {
        let cloned = msg.clone();
        prop_assert_eq!(msg.role, cloned.role);
        prop_assert_eq!(msg.content, cloned.content);
        prop_assert_eq!(msg.tool_call_id, cloned.tool_call_id);
    }

    #[test]
    fn stop_reason_display_not_empty(sr in arb_stop_reason()) {
        prop_assert!(!sr.to_string().is_empty());
    }
}
