use proptest::prelude::*;

use super::*;
use crate::provider::{ModelId, Role, ToolDefinition};
use crate::verify::{ChannelId, Message, MessageId};

// --- Fixtures ---

fn sample_message(author: &str, content: &str) -> Message {
    Message {
        channel_id: ChannelId("ch-1".into()),
        message_id: MessageId("msg-1".into()),
        timestamp: 1000,
        author: author.into(),
        content: content.into(),
    }
}

fn sample_context() -> Context {
    Context::new(ModelId::new("test-model"), 4096)
}

// --- Strategy helpers ---

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::System),
        Just(Role::User),
        Just(Role::Assistant),
        Just(Role::Tool),
    ]
}

fn arb_message() -> impl Strategy<Value = Message> {
    (
        "[0-9]{17,20}",
        "[0-9]{17,20}",
        any::<u64>(),
        "[a-zA-Z0-9_]{1,32}",
        ".*",
    )
        .prop_map(
            |(channel_id, message_id, timestamp, author, content)| Message {
                channel_id: ChannelId(channel_id),
                message_id: MessageId(message_id),
                timestamp,
                author,
                content,
            },
        )
}

fn arb_model_id() -> impl Strategy<Value = ModelId> {
    "[a-z0-9-]{1,40}".prop_map(ModelId::new)
}

fn arb_tool_definition() -> impl Strategy<Value = ToolDefinition> {
    ("[a-z_]{1,20}", ".*", "\\{.*\\}").prop_map(|(name, desc, schema)| ToolDefinition {
        name,
        description: desc,
        input_schema: schema,
    })
}

fn arb_context_message() -> impl Strategy<Value = ContextMessage> {
    (arb_message(), arb_role()).prop_map(|(msg, role)| ContextMessage::new(msg, role))
}

// --- Construction tests ---

#[test]
fn new_context_is_empty() {
    let ctx = sample_context();
    assert!(ctx.is_empty());
    assert_eq!(ctx.len(), 0);
    assert!(ctx.system_prompt().is_none());
    assert!(ctx.tools().is_empty());
    assert_eq!(ctx.max_tokens(), 4096);
    assert!(ctx.temperature().is_none());
}

#[test]
fn set_and_clear_system_prompt() {
    let mut ctx = sample_context();
    ctx.set_system_prompt("You are helpful.");
    assert_eq!(ctx.system_prompt(), Some("You are helpful."));

    ctx.clear_system_prompt();
    assert!(ctx.system_prompt().is_none());
}

#[test]
fn set_temperature() {
    let mut ctx = sample_context();
    ctx.set_temperature(Some(0.7));
    assert_eq!(ctx.temperature(), Some(0.7));

    ctx.set_temperature(None);
    assert!(ctx.temperature().is_none());
}

#[test]
fn set_model() {
    let mut ctx = sample_context();
    ctx.set_model(ModelId::new("other-model"));
    assert_eq!(ctx.model().0, "other-model");
}

#[test]
fn set_max_tokens() {
    let mut ctx = sample_context();
    ctx.set_max_tokens(8192);
    assert_eq!(ctx.max_tokens(), 8192);
}

#[test]
fn push_messages() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", "hello"), Role::User);
    ctx.push(sample_message("bot", "hi there"), Role::Assistant);

    assert_eq!(ctx.len(), 2);
    assert!(!ctx.is_empty());
    assert_eq!(ctx.messages()[0].role, Role::User);
    assert_eq!(ctx.messages()[0].source.content, "hello");
    assert_eq!(ctx.messages()[1].role, Role::Assistant);
}

#[test]
fn push_context_message() {
    let mut ctx = sample_context();
    let cm = ContextMessage::new(sample_message("alice", "test"), Role::User);
    ctx.push_context_message(cm);
    assert_eq!(ctx.len(), 1);
}

#[test]
fn clear_messages_keeps_config() {
    let mut ctx = sample_context();
    ctx.set_system_prompt("sys");
    ctx.set_temperature(Some(0.5));
    ctx.push(sample_message("alice", "hello"), Role::User);

    ctx.clear_messages();
    assert!(ctx.is_empty());
    assert_eq!(ctx.system_prompt(), Some("sys"));
    assert_eq!(ctx.temperature(), Some(0.5));
}

#[test]
fn set_tools() {
    let mut ctx = sample_context();
    let tools = vec![ToolDefinition {
        name: "search".into(),
        description: "Search the web".into(),
        input_schema: r#"{"type":"object"}"#.into(),
    }];
    ctx.set_tools(tools);
    assert_eq!(ctx.tools().len(), 1);
    assert_eq!(ctx.tools()[0].name, "search");
}

// --- Completion request conversion ---

#[test]
fn to_completion_request_empty() {
    let ctx = sample_context();
    let req = ctx.to_completion_request();
    assert!(req.messages.is_empty());
    assert!(req.tools.is_none());
    assert_eq!(req.model.0, "test-model");
    assert_eq!(req.max_tokens, 4096);
}

#[test]
fn to_completion_request_with_system_prompt() {
    let mut ctx = sample_context();
    ctx.set_system_prompt("You are helpful.");
    ctx.push(sample_message("alice", "hello"), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0].role, Role::System);
    assert_eq!(req.messages[0].content, "You are helpful.");
    assert_eq!(req.messages[1].role, Role::User);
    assert_eq!(req.messages[1].content, "hello");
}

#[test]
fn to_completion_request_without_system_prompt() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", "hello"), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, Role::User);
}

#[test]
fn to_completion_request_preserves_order() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", "first"), Role::User);
    ctx.push(sample_message("bot", "second"), Role::Assistant);
    ctx.push(sample_message("alice", "third"), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[0].content, "first");
    assert_eq!(req.messages[1].content, "second");
    assert_eq!(req.messages[2].content, "third");
}

#[test]
fn to_completion_request_with_tools() {
    let mut ctx = sample_context();
    ctx.set_tools(vec![ToolDefinition {
        name: "calc".into(),
        description: "Calculator".into(),
        input_schema: r#"{"type":"object"}"#.into(),
    }]);
    ctx.push(sample_message("alice", "what is 2+2?"), Role::User);

    let req = ctx.to_completion_request();
    assert!(req.tools.is_some());
    assert_eq!(req.tools.as_ref().unwrap().len(), 1);
    assert_eq!(req.tools.as_ref().unwrap()[0].name, "calc");
}

#[test]
fn to_completion_request_empty_tools_is_none() {
    let ctx = sample_context();
    let req = ctx.to_completion_request();
    assert!(req.tools.is_none());
}

#[test]
fn to_completion_request_carries_temperature() {
    let mut ctx = sample_context();
    ctx.set_temperature(Some(1.5));
    let req = ctx.to_completion_request();
    assert_eq!(req.temperature, Some(1.5));
}

#[test]
fn to_completion_request_carries_model_and_max_tokens() {
    let mut ctx = Context::new(ModelId::new("custom-model"), 2048);
    ctx.push(sample_message("alice", "hi"), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.model.0, "custom-model");
    assert_eq!(req.max_tokens, 2048);
}

// --- ContextMessage tests ---

#[test]
fn context_message_preserves_source() {
    let msg = sample_message("alice", "hello");
    let cm = ContextMessage::new(msg.clone(), Role::User);
    assert_eq!(cm.source, msg);
    assert_eq!(cm.role, Role::User);
}

#[test]
fn context_message_clone_eq() {
    let cm = ContextMessage::new(sample_message("alice", "hello"), Role::User);
    let cloned = cm.clone();
    assert_eq!(cm, cloned);
}

// --- Edge cases ---

#[test]
fn tool_role_messages() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", "use the tool"), Role::User);
    ctx.push(
        sample_message("bot", r#"{"id":"1","name":"calc"}"#),
        Role::Assistant,
    );
    ctx.push(sample_message("system", r#"{"result": 42}"#), Role::Tool);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[2].role, Role::Tool);
    assert_eq!(req.messages[2].content, r#"{"result": 42}"#);
}

#[test]
fn multiple_system_prompt_overwrites() {
    let mut ctx = sample_context();
    ctx.set_system_prompt("first");
    ctx.set_system_prompt("second");
    assert_eq!(ctx.system_prompt(), Some("second"));

    let req = ctx.to_completion_request();
    // Only one system message even though we set the prompt twice.
    let system_count = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .count();
    assert_eq!(system_count, 1);
    assert_eq!(req.messages[0].content, "second");
}

#[test]
fn empty_content_message() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", ""), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].content, "");
}

#[test]
fn unicode_content_preserved() {
    let mut ctx = sample_context();
    ctx.push(sample_message("alice", "こんにちは 🌸"), Role::User);

    let req = ctx.to_completion_request();
    assert_eq!(req.messages[0].content, "こんにちは 🌸");
}

#[test]
fn max_tokens_zero() {
    let ctx = Context::new(ModelId::new("m"), 0);
    let req = ctx.to_completion_request();
    assert_eq!(req.max_tokens, 0);
}

// --- Proptests ---

proptest! {
    #[test]
    fn push_then_len(msgs in prop::collection::vec(arb_context_message(), 0..=20)) {
        let mut ctx = sample_context();
        for cm in &msgs {
            ctx.push_context_message(cm.clone());
        }
        prop_assert_eq!(ctx.len(), msgs.len());
    }

    #[test]
    fn completion_request_message_count(
        system in prop::option::of(".*"),
        msgs in prop::collection::vec((arb_message(), arb_role()), 0..=10),
    ) {
        let mut ctx = sample_context();
        if let Some(ref s) = system {
            ctx.set_system_prompt(s.clone());
        }
        for (msg, role) in &msgs {
            ctx.push(msg.clone(), *role);
        }
        let req = ctx.to_completion_request();
        let expected = msgs.len() + usize::from(system.is_some());
        prop_assert_eq!(req.messages.len(), expected);
    }

    #[test]
    fn completion_request_preserves_content(
        msgs in prop::collection::vec((arb_message(), arb_role()), 1..=10),
    ) {
        let mut ctx = sample_context();
        for (msg, role) in &msgs {
            ctx.push(msg.clone(), *role);
        }
        let req = ctx.to_completion_request();
        for (i, (msg, role)) in msgs.iter().enumerate() {
            prop_assert_eq!(&req.messages[i].content, &msg.content);
            prop_assert_eq!(req.messages[i].role, *role);
        }
    }

    #[test]
    fn clear_resets_length(msgs in prop::collection::vec(arb_context_message(), 1..=10)) {
        let mut ctx = sample_context();
        for cm in msgs {
            ctx.push_context_message(cm);
        }
        ctx.clear_messages();
        prop_assert!(ctx.is_empty());
        prop_assert_eq!(ctx.len(), 0);
    }

    #[test]
    fn model_roundtrip(model in arb_model_id(), max_tokens in 1u32..=200_000u32) {
        let ctx = Context::new(model.clone(), max_tokens);
        prop_assert_eq!(&ctx.model().0, &model.0);
        prop_assert_eq!(ctx.max_tokens(), max_tokens);
    }

    #[test]
    fn tools_none_when_empty(msgs in prop::collection::vec((arb_message(), arb_role()), 0..=5)) {
        let mut ctx = sample_context();
        for (msg, role) in msgs {
            ctx.push(msg, role);
        }
        let req = ctx.to_completion_request();
        prop_assert!(req.tools.is_none());
    }

    #[test]
    fn tools_some_when_present(
        tools in prop::collection::vec(arb_tool_definition(), 1..=5),
    ) {
        let mut ctx = sample_context();
        ctx.set_tools(tools.clone());
        let req = ctx.to_completion_request();
        prop_assert!(req.tools.is_some());
        prop_assert_eq!(req.tools.unwrap().len(), tools.len());
    }

    #[test]
    fn context_message_clone_preserves_fields(cm in arb_context_message()) {
        let cloned = cm.clone();
        prop_assert_eq!(cm.source, cloned.source);
        prop_assert_eq!(cm.role, cloned.role);
    }

    #[test]
    fn system_prompt_in_first_position(
        prompt in ".*",
        msgs in prop::collection::vec((arb_message(), arb_role()), 1..=5),
    ) {
        let mut ctx = sample_context();
        ctx.set_system_prompt(prompt.clone());
        for (msg, role) in &msgs {
            ctx.push(msg.clone(), *role);
        }
        let req = ctx.to_completion_request();
        prop_assert_eq!(req.messages[0].role, Role::System);
        prop_assert_eq!(&req.messages[0].content, &prompt);
    }
}
