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
    pub fn parse_json(payload: &Value) -> Result<Self, AppError> {
        let object = payload.as_object().ok_or_else(|| {
            AppError::BadRequest("anthropic payload must be a json object".to_string())
        })?;
        let model = required_string(
            object.get("model"),
            "field `model` is required",
            "field `model` must be a string",
        )?;
        let messages = object
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::BadRequest("field `messages` is required".to_string()))?
            .iter()
            .map(parse_message)
            .collect::<Result<Vec<_>, _>>()?;
        let system = object.get("system").map(parse_system_prompt).transpose()?;
        let stream = object
            .get("stream")
            .map(value_as_bool)
            .transpose()?
            .unwrap_or(false);
        let max_tokens = object
            .get("max_tokens")
            .map(|value| value_as_u64(value, "field `max_tokens` must be an integer"))
            .transpose()?;
        let temperature = object
            .get("temperature")
            .map(|value| value_as_f64(value, "field `temperature` must be a number"))
            .transpose()?;
        let top_p = object
            .get("top_p")
            .map(|value| value_as_f64(value, "field `top_p` must be a number"))
            .transpose()?;
        let tools = match object.get("tools") {
            Some(value) => value
                .as_array()
                .ok_or_else(|| AppError::BadRequest("field `tools` must be an array".to_string()))?
                .iter()
                .map(parse_tool_definition)
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let tool_choice = object
            .get("tool_choice")
            .map(parse_tool_choice)
            .transpose()?;

        Ok(Self {
            model,
            messages,
            system,
            stream,
            max_tokens,
            temperature,
            top_p,
            tools,
            tool_choice,
            extensions: ClaudeRequestExtensions {
                thinking: object.get("thinking").cloned(),
                context_management: object.get("context_management").cloned(),
                metadata: object.get("metadata").cloned(),
                beta_hints: object
                    .get("beta")
                    .cloned()
                    .or_else(|| object.get("betas").cloned()),
                request_hints: object
                    .get("request_hints")
                    .cloned()
                    .or_else(|| object.get("service_tier").cloned()),
            },
        })
    }

    pub fn system_text(&self) -> Option<String> {
        self.system
            .as_ref()
            .map(ClaudeSystemPrompt::normalized_text)
    }

    pub fn has_plaintext_assistant_history(&self) -> bool {
        self.messages
            .iter()
            .any(ClaudeMessage::is_plaintext_assistant)
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

    pub fn is_plaintext_assistant(&self) -> bool {
        self.role == ClaudeRole::Assistant
            && !self.content.is_empty()
            && self
                .content
                .iter()
                .all(|block| matches!(block, ClaudeContentBlock::Text(ClaudeTextBlock { text }) if !text.trim().is_empty()))
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

fn parse_system_prompt(value: &Value) -> Result<ClaudeSystemPrompt, AppError> {
    match value {
        Value::String(text) => Ok(ClaudeSystemPrompt {
            blocks: vec![ClaudeTextBlock { text: text.clone() }],
        }),
        Value::Array(blocks) => Ok(ClaudeSystemPrompt {
            blocks: blocks
                .iter()
                .map(parse_text_like_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        _ => Err(AppError::BadRequest(
            "field `system` must be a string or array".to_string(),
        )),
    }
}

fn parse_message(value: &Value) -> Result<ClaudeMessage, AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::BadRequest("anthropic message must be an object".to_string()))?;
    let role = match required_string(
        object.get("role"),
        "anthropic message role is required",
        "anthropic message role must be a string",
    )?
    .as_str()
    {
        "user" => ClaudeRole::User,
        "assistant" => ClaudeRole::Assistant,
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported anthropic message role: {other}"
            )));
        }
    };
    let content = object
        .get("content")
        .ok_or_else(|| AppError::BadRequest("anthropic message content is required".to_string()))
        .and_then(parse_content_blocks)?;

    Ok(ClaudeMessage { role, content })
}

fn parse_content_blocks(value: &Value) -> Result<Vec<ClaudeContentBlock>, AppError> {
    match value {
        Value::String(text) => Ok(vec![ClaudeContentBlock::Text(ClaudeTextBlock {
            text: text.clone(),
        })]),
        Value::Array(blocks) => blocks.iter().map(parse_content_block).collect(),
        _ => Err(AppError::BadRequest(
            "anthropic message content must be a string or array".to_string(),
        )),
    }
}

fn parse_content_block(value: &Value) -> Result<ClaudeContentBlock, AppError> {
    let object = value.as_object().ok_or_else(|| {
        AppError::BadRequest("anthropic content block must be an object".to_string())
    })?;
    let block_type = required_string(
        object.get("type"),
        "anthropic content block type is required",
        "anthropic content block type must be a string",
    )?;

    match block_type.as_str() {
        "text" => Ok(ClaudeContentBlock::Text(parse_text_like_block(value)?)),
        "tool_use" => Ok(ClaudeContentBlock::ToolUse(ClaudeToolUse {
            id: required_string(
                object.get("id"),
                "anthropic tool_use id is required",
                "anthropic tool_use id must be a string",
            )?,
            name: required_string(
                object.get("name"),
                "anthropic tool_use name is required",
                "anthropic tool_use name must be a string",
            )?,
            input: object
                .get("input")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        })),
        "tool_result" => Ok(ClaudeContentBlock::ToolResult(ClaudeToolResult {
            tool_use_id: required_string(
                object.get("tool_use_id"),
                "anthropic tool_result tool_use_id is required",
                "anthropic tool_result tool_use_id must be a string",
            )?,
            content: parse_tool_result_content(object.get("content"))?,
            is_error: object
                .get("is_error")
                .map(value_as_bool)
                .transpose()?
                .unwrap_or(false),
        })),
        other => Err(AppError::BadRequest(format!(
            "unsupported anthropic content block type: {other}"
        ))),
    }
}

fn parse_text_like_block(value: &Value) -> Result<ClaudeTextBlock, AppError> {
    let object = value.as_object().ok_or_else(|| {
        AppError::BadRequest("anthropic text block must be an object".to_string())
    })?;
    let text = required_string(
        object.get("text"),
        "anthropic text block text is required",
        "anthropic text block text must be a string",
    )?;
    Ok(ClaudeTextBlock { text })
}

fn parse_tool_result_content(value: Option<&Value>) -> Result<ClaudeToolResultContent, AppError> {
    match value {
        None | Some(Value::Null) => Ok(ClaudeToolResultContent::Empty),
        Some(Value::String(text)) => Ok(ClaudeToolResultContent::Text(text.clone())),
        Some(Value::Array(blocks)) => Ok(ClaudeToolResultContent::TextBlocks(
            blocks
                .iter()
                .map(|block| {
                    parse_text_like_block(block).map_err(|_| {
                        AppError::BadRequest(
                            "anthropic tool_result blocks must contain text".to_string(),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Some(other) => Ok(ClaudeToolResultContent::Json(other.clone())),
    }
}

fn parse_tool_definition(value: &Value) -> Result<ClaudeToolDefinition, AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::BadRequest("anthropic tool must be an object".to_string()))?;
    Ok(ClaudeToolDefinition {
        name: required_string(
            object.get("name"),
            "anthropic tool name is required",
            "anthropic tool name must be a string",
        )?,
        description: optional_string(
            object.get("description"),
            "anthropic tool description must be a string",
        )?,
        input_schema: object.get("input_schema").cloned(),
    })
}

fn parse_tool_choice(value: &Value) -> Result<ClaudeToolChoice, AppError> {
    if let Some(kind) = value.as_str() {
        return parse_tool_choice_kind(kind, None);
    }

    let object = value.as_object().ok_or_else(|| {
        AppError::BadRequest("field `tool_choice` must be a string or object".to_string())
    })?;
    let kind = required_string(
        object.get("type"),
        "anthropic tool_choice.type is required",
        "anthropic tool_choice.type must be a string",
    )?;
    parse_tool_choice_kind(kind.as_str(), object.get("name"))
}

fn parse_tool_choice_kind(kind: &str, name: Option<&Value>) -> Result<ClaudeToolChoice, AppError> {
    match kind {
        "auto" => Ok(ClaudeToolChoice::Auto),
        "any" => Ok(ClaudeToolChoice::Any),
        "tool" => Ok(ClaudeToolChoice::Tool {
            name: required_string(
                name,
                "anthropic tool_choice.name is required",
                "anthropic tool_choice.name must be a string",
            )?,
        }),
        other => Err(AppError::BadRequest(format!(
            "unsupported anthropic tool_choice.type: {other}"
        ))),
    }
}

fn required_string(
    value: Option<&Value>,
    missing_message: &str,
    type_message: &str,
) -> Result<String, AppError> {
    let Some(value) = value else {
        return Err(AppError::BadRequest(missing_message.to_string()));
    };
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| AppError::BadRequest(type_message.to_string()))
}

fn optional_string(value: Option<&Value>, type_message: &str) -> Result<Option<String>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_string()))
            .ok_or_else(|| AppError::BadRequest(type_message.to_string())),
    }
}

fn value_as_bool(value: &Value) -> Result<bool, AppError> {
    value
        .as_bool()
        .ok_or_else(|| AppError::BadRequest("field value must be a boolean".to_string()))
}

fn value_as_u64(value: &Value, message: &str) -> Result<u64, AppError> {
    value
        .as_u64()
        .ok_or_else(|| AppError::BadRequest(message.to_string()))
}

fn value_as_f64(value: &Value, message: &str) -> Result<f64, AppError> {
    value
        .as_f64()
        .ok_or_else(|| AppError::BadRequest(message.to_string()))
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
        assert_eq!(
            request.system_text().as_deref(),
            Some("you are helpful\nstay terse")
        );
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

        assert!(
            matches!(error, AppError::BadRequest(message) if message.contains("unsupported anthropic message role"))
        );
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

        assert!(
            matches!(error, AppError::BadRequest(message) if message.contains("anthropic tool_use id is required"))
        );
    }

    #[test]
    fn claude_semantic_core_requires_model_and_messages() {
        let error = ClaudeMessageRequest::parse_json(&json!({ "stream": false }))
            .expect_err("missing required fields should fail");

        assert!(
            matches!(error, AppError::BadRequest(message) if message.contains("field `model` is required"))
        );
    }
}
