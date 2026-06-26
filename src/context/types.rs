use crate::provider::{ChatMessage, ModelId, Role, ToolDefinition};
use crate::verify::Message;

/// A verified transport message annotated with its conversation role.
///
/// Preserves the original [`Message`] (with channel/message IDs, timestamp,
/// signature provenance) alongside the [`Role`] it plays in the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMessage {
    /// The verified message from transport.
    pub source: Message,
    /// The role this message plays in the conversation.
    pub role: Role,
}

/// Conversation context that bridges verified transport messages to inference
/// requests.
///
/// Accumulates [`ContextMessage`]s and converts them into a
/// [`CompletionRequest`](crate::provider::CompletionRequest) ready for an
/// [`InferenceProvider`](crate::provider::InferenceProvider).
///
/// # Example
///
/// ```
/// use helene::context::Context;
/// use helene::provider::{ModelId, Role};
/// use helene::verify::{ChannelId, Message, MessageId};
///
/// let mut ctx = Context::new(ModelId::new("claude-opus-4-20250514"), 4096);
/// ctx.set_system_prompt("You are helpful.");
///
/// let msg = Message {
///     channel_id: ChannelId("123".into()),
///     message_id: MessageId("456".into()),
///     timestamp: 1000,
///     author: "alice".into(),
///     content: "Hello!".into(),
/// };
/// ctx.push(msg, Role::User);
///
/// let req = ctx.to_completion_request();
/// assert_eq!(req.messages.len(), 2); // system + user
/// ```
#[derive(Debug, Clone)]
pub struct Context {
    system_prompt: Option<String>,
    model: ModelId,
    max_tokens: u32,
    temperature: Option<f64>,
    tools: Vec<ToolDefinition>,
    messages: Vec<ContextMessage>,
}

impl ContextMessage {
    /// Create a new context message from a verified transport message.
    pub fn new(source: Message, role: Role) -> Self {
        Self { source, role }
    }
}

impl Context {
    /// Create a new context with the given model and token limit.
    pub fn new(model: ModelId, max_tokens: u32) -> Self {
        Self {
            system_prompt: None,
            model,
            max_tokens,
            temperature: None,
            tools: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    /// Clear the system prompt.
    pub fn clear_system_prompt(&mut self) {
        self.system_prompt = None;
    }

    /// Return the current system prompt, if set.
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Set the sampling temperature. `None` uses the provider default.
    pub fn set_temperature(&mut self, temperature: Option<f64>) {
        self.temperature = temperature;
    }

    /// Return the current temperature setting.
    pub fn temperature(&self) -> Option<f64> {
        self.temperature
    }

    /// Set the model.
    pub fn set_model(&mut self, model: ModelId) {
        self.model = model;
    }

    /// Return the current model.
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// Set the maximum output tokens.
    pub fn set_max_tokens(&mut self, max_tokens: u32) {
        self.max_tokens = max_tokens;
    }

    /// Return the current maximum output tokens.
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    /// Replace the tool definitions.
    pub fn set_tools(&mut self, tools: Vec<ToolDefinition>) {
        self.tools = tools;
    }

    /// Return the current tool definitions.
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    /// Append a verified message with the given role to the conversation.
    pub fn push(&mut self, message: Message, role: Role) {
        self.messages.push(ContextMessage::new(message, role));
    }

    /// Append a pre-built [`ContextMessage`].
    pub fn push_context_message(&mut self, ctx_msg: ContextMessage) {
        self.messages.push(ctx_msg);
    }

    /// Return the conversation messages in order.
    pub fn messages(&self) -> &[ContextMessage] {
        &self.messages
    }

    /// Number of messages in the conversation (excludes system prompt).
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the conversation has no messages (excludes system prompt).
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Remove all messages, keeping configuration (model, system prompt, tools).
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Build a [`CompletionRequest`](crate::provider::CompletionRequest) from
    /// this context.
    ///
    /// The system prompt (if set) is prepended as the first message with
    /// [`Role::System`]. Verified messages follow in insertion order, with
    /// their content mapped to [`ChatMessage`]s.
    pub fn to_completion_request(&self) -> crate::provider::CompletionRequest {
        let mut chat_messages =
            Vec::with_capacity(self.messages.len() + usize::from(self.system_prompt.is_some()));

        if let Some(ref prompt) = self.system_prompt {
            chat_messages.push(ChatMessage::system(prompt.clone()));
        }

        for ctx_msg in &self.messages {
            chat_messages.push(ChatMessage::new(
                ctx_msg.role,
                ctx_msg.source.content.clone(),
            ));
        }

        let tools = if self.tools.is_empty() {
            None
        } else {
            Some(self.tools.clone())
        };

        crate::provider::CompletionRequest {
            model: self.model.clone(),
            messages: chat_messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            tools,
        }
    }
}
