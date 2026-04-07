use serde_json::Value;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeMessageRequest {
    pub model: String,
    pub messages: Vec<ClaudeMessage>,
    pub system: Option<ClaudeSystemPrompt>,
    pub stream: bool,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub tools: Vec<ClaudeToolDefinition>,
    pub tool_choice: Option<ClaudeToolChoice>,
    pub extensions: ClaudeRequestExtensions,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSystemPrompt {
    pub blocks: Vec<ClaudeTextBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeMessage {
    pub role: ClaudeRole,
    pub content: Vec<ClaudeContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeContentBlock {
    Text(ClaudeTextBlock),
    ToolUse(ClaudeToolUse),
    ToolResult(ClaudeToolResult),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeTextBlock {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeToolResult {
    pub tool_use_id: String,
    pub content: ClaudeToolResultContent,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeToolResultContent {
    Empty,
    Text(String),
    TextBlocks(Vec<ClaudeTextBlock>),
    Json(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeRequestExtensions {
    pub thinking: Option<Value>,
    pub context_management: Option<Value>,
    pub metadata: Option<Value>,
    pub beta_hints: Option<Value>,
    pub request_hints: Option<Value>,
}

impl ClaudeMessageRequest {
    pub fn parse_json(_payload: &Value) -> Result<Self, AppError> {
        Err(AppError::BadRequest(
            "claude semantic parsing not implemented".to_string(),
        ))
    }

    pub fn system_text(&self) -> Option<String> {
        self.system.as_ref().map(ClaudeSystemPrompt::normalized_text)
    }

    pub fn has_plaintext_assistant_history(&self) -> bool {
        false
    }
}

impl ClaudeSystemPrompt {
    pub fn normalized_text(&self) -> String {
        self.blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl ClaudeMessage {
    pub fn text_fragments(&self) -> Vec<String> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ClaudeContentBlock::Text(block) if !block.text.trim().is_empty() => {
                    Some(block.text.clone())
                }
                _ => None,
            })
            .collect()
    }
}

impl ClaudeToolResultContent {
    pub fn as_joined_text(&self) -> Option<String> {
        match self {
            Self::Empty => Some(String::new()),
            Self::Text(text) => Some(text.clone()),
            Self::TextBlocks(blocks) => Some(
                blocks
                    .iter()
                    .map(|block| block.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Self::Json(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claude_semantic_core_parses_supported_message_slice() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "system": [
                { "type": "text", "text": "you are helpful" },
                { "type": "text", "text": "stay terse" }
            ],
            "stream": true,
            "max_tokens": 256,
            "temperature": 0.2,
            "top_p": 0.9,
            "tools": [{
                "name": "lookup_weather",
                "description": "Fetch weather",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }],
            "tool_choice": { "type": "tool", "name": "lookup_weather" },
            "thinking": { "type": "enabled" },
            "context_management": { "strategy": "retain" },
            "metadata": { "tenant": "ops" },
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "weather in Paris?" },
                        {
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": [{ "type": "text", "text": "sunny" }]
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "Let me check." },
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "lookup_weather",
                            "input": { "city": "Paris" }
                        }
                    ]
                }
            ]
        }))
        .expect("request should parse");

        assert_eq!(request.model, "claude-sonnet-4-6");
        assert!(request.stream);
        assert_eq!(request.max_tokens, Some(256));
        assert_eq!(request.system_text().as_deref(), Some("you are helpful\nstay terse"));
        assert_eq!(request.tools.len(), 1);
        assert_eq!(
            request.tool_choice,
            Some(ClaudeToolChoice::Tool {
                name: "lookup_weather".to_string()
            })
        );
        assert!(request.extensions.thinking.is_some());
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, ClaudeRole::User);
        assert_eq!(request.messages[1].role, ClaudeRole::Assistant);
        assert!(matches!(
            &request.messages[0].content[0],
            ClaudeContentBlock::Text(ClaudeTextBlock { text }) if text == "weather in Paris?"
        ));
        assert!(matches!(
            &request.messages[1].content[1],
            ClaudeContentBlock::ToolUse(ClaudeToolUse { id, name, .. }) if id == "call_1" && name == "lookup_weather"
        ));
    }

    #[test]
    fn claude_semantic_core_rejects_unsupported_roles() {
        let error = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "system", "content": "hi" }]
        }))
        .expect_err("invalid roles should fail");

        assert!(matches!(error, AppError::BadRequest(message) if message.contains("unsupported anthropic message role")));
    }

    #[test]
    fn claude_semantic_core_rejects_malformed_content_blocks() {
        let error = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [{ "type": "tool_use", "name": "lookup_weather" }]
            }]
        }))
        .expect_err("malformed blocks should fail");

        assert!(matches!(error, AppError::BadRequest(message) if message.contains("anthropic tool_use id is required")));
    }

    #[test]
    fn claude_semantic_core_requires_model_and_messages() {
        let error = ClaudeMessageRequest::parse_json(&json!({ "stream": false }))
            .expect_err("missing required fields should fail");

        assert!(matches!(error, AppError::BadRequest(message) if message.contains("field `model` is required")));
    }
}
