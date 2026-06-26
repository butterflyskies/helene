use std::fmt;

/// Unique identifier for a model (e.g. `claude-opus-4-20250514`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(pub String);

/// A role in an inference conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// For [`Role::Tool`] messages, the ID of the tool call this result belongs to.
    pub tool_call_id: Option<String>,
}

/// A tool available to the model during inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input parameters, stored as a raw string.
    pub input_schema: String,
}

/// A tool invocation returned by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: String,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
}

/// Token usage statistics for a single completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// The content returned by a model completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseContent {
    Text(String),
    ToolCalls(Vec<ToolCall>),
}

/// A request to a model for completion.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub model: ModelId,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    pub tools: Option<Vec<ToolDefinition>>,
}

/// A model's response to a completion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    pub content: ResponseContent,
    pub model: ModelId,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

// --- Display impls (matching verifier pattern) ---

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::System => f.write_str("system"),
            Role::User => f.write_str("user"),
            Role::Assistant => f.write_str("assistant"),
            Role::Tool => f.write_str("tool"),
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::EndTurn => f.write_str("end_turn"),
            StopReason::MaxTokens => f.write_str("max_tokens"),
            StopReason::ToolUse => f.write_str("tool_use"),
            StopReason::StopSequence => f.write_str("stop_sequence"),
        }
    }
}

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}in/{}out", self.input_tokens, self.output_tokens)
    }
}

// --- Constructors ---

impl ModelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    pub fn tool(content: impl Into<String>) -> Self {
        Self::new(Role::Tool, content)
    }

    /// Create a tool-result message tied to a specific tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

impl Usage {
    pub fn new(input_tokens: u32, output_tokens: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }

    pub fn total(&self) -> u32 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}
