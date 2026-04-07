use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::{
    claude::provider_capability_profile::{
        ClaudeExtensionResolution, ClaudeProviderCapabilityProfile,
    },
    claude::semantic_core::{
        ClaudeContentBlock, ClaudeMessage, ClaudeMessageRequest, ClaudeRole, ClaudeToolChoice,
        ClaudeToolDefinition, ClaudeToolResult, ClaudeToolResultContent, ClaudeToolUse,
    },
    error::AppError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponsesRequestMode {
    #[default]
    Standard,
    AssistantHistoryCompat,
}

#[derive(Debug, Clone)]
pub struct ResponsesProviderAdapter {
    request_mode: ResponsesRequestMode,
    capability_profile: ClaudeProviderCapabilityProfile,
}

pub struct AnthropicStreamEventAdapter {
    requested_model: String,
    request_id: String,
    state: AnthropicStreamState,
}

#[derive(Debug, Default, Clone)]
struct AnthropicStreamState {
    sent_message_start: bool,
    text_block_index: Option<usize>,
    text_block_closed: bool,
    saw_tool_use: bool,
    finalized: bool,
    next_block_index: usize,
    tool_blocks: HashMap<i64, AnthropicToolBlockState>,
}

#[derive(Debug, Clone)]
struct AnthropicToolBlockState {
    block_index: usize,
    call_id: String,
    name: String,
    started: bool,
    closed: bool,
}

impl ResponsesProviderAdapter {
    pub fn new() -> Self {
        Self {
            request_mode: ResponsesRequestMode::default(),
            capability_profile: ClaudeProviderCapabilityProfile::responses(),
        }
    }

    pub fn with_request_mode(mut self, request_mode: ResponsesRequestMode) -> Self {
        self.request_mode = request_mode;
        self
    }

    pub fn with_capability_profile(
        mut self,
        capability_profile: ClaudeProviderCapabilityProfile,
    ) -> Self {
        self.capability_profile = capability_profile;
        self
    }

    pub fn request_mode(&self) -> ResponsesRequestMode {
        self.request_mode
    }

    pub fn capability_profile(&self) -> &ClaudeProviderCapabilityProfile {
        &self.capability_profile
    }

    pub fn should_retry_with_assistant_history_compat(
        &self,
        request: &ClaudeMessageRequest,
    ) -> bool {
        self.capability_profile
            .supports_assistant_history_compat_retry()
            && request.has_plaintext_assistant_history()
    }

    pub fn extension_policy(&self, request: &ClaudeMessageRequest) -> ClaudeExtensionResolution {
        self.capability_profile
            .resolve_extensions(&request.extensions)
    }

    pub fn request_to_payload(&self, request: &ClaudeMessageRequest) -> Result<Value, AppError> {
        let assistant_text_mode = match self.request_mode {
            ResponsesRequestMode::Standard => AssistantTextHistoryMode::NativeRole,
            ResponsesRequestMode::AssistantHistoryCompat => {
                AssistantTextHistoryMode::TranscriptUser
            }
        };
        let extension_policy = self.extension_policy(request);
        let mut body = Map::new();
        body.insert(
            "model".to_string(),
            Value::String(request.model.to_string()),
        );
        body.insert(
            "input".to_string(),
            Value::Array(messages_to_responses_input(
                &request.messages,
                assistant_text_mode,
            )?),
        );
        body.insert("stream".to_string(), Value::Bool(request.stream));

        if let Some(temperature) = request.temperature {
            body.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = request.top_p {
            body.insert("top_p".to_string(), json!(top_p));
        }
        if let Some(max_tokens) = request.max_tokens {
            body.insert("max_output_tokens".to_string(), json!(max_tokens));
        }
        if let Some(system_text) = request.system_text().filter(|text| !text.is_empty()) {
            body.insert("instructions".to_string(), Value::String(system_text));
        }
        if !request.tools.is_empty() {
            body.insert(
                "tools".to_string(),
                Value::Array(
                    request
                        .tools
                        .iter()
                        .map(map_tool_definition)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            );
        }
        if let Some(tool_choice) = &request.tool_choice {
            body.insert(
                "tool_choice".to_string(),
                map_tool_choice(tool_choice.clone())?,
            );
        }
        if let Some(metadata) = extension_policy.metadata.forwarded {
            body.insert("metadata".to_string(), Value::Object(metadata));
        }
        if let Some(service_tier) = extension_policy.service_tier.forwarded {
            body.insert("service_tier".to_string(), Value::String(service_tier));
        }

        Ok(Value::Object(body))
    }

    pub fn response_to_message(
        &self,
        response: &Value,
        requested_model: &str,
        request_id: &str,
    ) -> Result<Value, AppError> {
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("msg_{request_id}"));
        let content = extract_response_output_text(response);
        let tool_calls = extract_response_tool_calls(response);
        let stop_reason = if tool_calls.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        };

        let mut content_blocks = Vec::new();
        if !content.is_empty() {
            content_blocks.push(json!({
                "type": "text",
                "text": content
            }));
        }
        for tool_call in tool_calls {
            let name = tool_call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = tool_call
                .get("function")
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
            content_blocks.push(json!({
                "type": "tool_use",
                "id": tool_call.get("id").and_then(Value::as_str).unwrap_or_default(),
                "name": name,
                "input": input
            }));
        }

        Ok(json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": requested_model,
            "content": content_blocks,
            "stop_reason": stop_reason,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": response.get("usage").and_then(|usage| usage.get("input_tokens")).and_then(Value::as_i64).unwrap_or(0),
                "output_tokens": response.get("usage").and_then(|usage| usage.get("output_tokens")).and_then(Value::as_i64).unwrap_or(0)
            }
        }))
    }

    pub fn stream_event_adapter(
        &self,
        requested_model: impl Into<String>,
        request_id: impl Into<String>,
    ) -> AnthropicStreamEventAdapter {
        AnthropicStreamEventAdapter {
            requested_model: requested_model.into(),
            request_id: request_id.into(),
            state: AnthropicStreamState::default(),
        }
    }
}

impl AnthropicStreamEventAdapter {
    pub fn translate_frame(&mut self, frame: &str) -> Result<Vec<String>, String> {
        let Some(data) = extract_sse_data(frame) else {
            return Ok(Vec::new());
        };

        if data == "[DONE]" {
            let stop_reason = if self.state.saw_tool_use {
                "tool_use"
            } else {
                "end_turn"
            };
            return Ok(finalize_anthropic_stream(
                &mut self.state,
                Some(stop_reason),
            ));
        }

        let event: Value = serde_json::from_str(&data)
            .map_err(|error| format!("invalid upstream sse json: {error}"))?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "response.created" => Ok(ensure_anthropic_message_start(
                &self.requested_model,
                &self.request_id,
                &mut self.state,
            )),
            "response.output_text.delta" => {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut lines = ensure_anthropic_message_start(
                    &self.requested_model,
                    &self.request_id,
                    &mut self.state,
                );
                if delta.is_empty() {
                    return Ok(lines);
                }
                let block_index = ensure_anthropic_text_block(&mut self.state, &mut lines);
                lines.push(anthropic_content_block_delta_line(
                    block_index,
                    json!({ "type": "text_delta", "text": delta }),
                ));
                Ok(lines)
            }
            "response.output_item.added" => {
                let item = event.get("item").ok_or("missing response output item")?;
                if item.get("type").and_then(Value::as_str) != Some("function_call") {
                    return Ok(Vec::new());
                }

                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("id").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                let mut lines = ensure_anthropic_message_start(
                    &self.requested_model,
                    &self.request_id,
                    &mut self.state,
                );
                close_anthropic_text_block(&mut self.state, &mut lines);
                let mut saw_tool_use = false;
                let block_index;
                {
                    let block =
                        anthropic_tool_block_state(output_index, call_id, name, &mut self.state);
                    block_index = block.block_index;
                    if !block.started {
                        lines.push(anthropic_content_block_start_line(
                            block.block_index,
                            json!({
                                "type": "tool_use",
                                "id": block.call_id,
                                "name": block.name,
                                "input": {}
                            }),
                        ));
                        block.started = true;
                        saw_tool_use = true;
                    }
                }
                if saw_tool_use {
                    self.state.saw_tool_use = true;
                }
                if !arguments.is_empty() {
                    lines.push(anthropic_content_block_delta_line(
                        block_index,
                        json!({ "type": "input_json_delta", "partial_json": arguments }),
                    ));
                }
                Ok(lines)
            }
            "response.function_call_arguments.delta" => {
                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if delta.is_empty() {
                    return Ok(Vec::new());
                }

                let mut lines = ensure_anthropic_message_start(
                    &self.requested_model,
                    &self.request_id,
                    &mut self.state,
                );
                close_anthropic_text_block(&mut self.state, &mut lines);
                let mut saw_tool_use = false;
                let block_index;
                {
                    let block = anthropic_tool_block_state(
                        output_index,
                        format!("call_{output_index}"),
                        String::new(),
                        &mut self.state,
                    );
                    block_index = block.block_index;
                    if !block.started {
                        lines.push(anthropic_content_block_start_line(
                            block.block_index,
                            json!({
                                "type": "tool_use",
                                "id": block.call_id,
                                "name": block.name,
                                "input": {}
                            }),
                        ));
                        block.started = true;
                        saw_tool_use = true;
                    }
                }
                if saw_tool_use {
                    self.state.saw_tool_use = true;
                }
                lines.push(anthropic_content_block_delta_line(
                    block_index,
                    json!({ "type": "input_json_delta", "partial_json": delta }),
                ));
                Ok(lines)
            }
            "response.function_call_arguments.done" => {
                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let mut lines = Vec::new();
                if let Some(block) = self.state.tool_blocks.get_mut(&output_index) {
                    if !block.closed {
                        lines.push(anthropic_content_block_stop_line(block.block_index));
                        block.closed = true;
                    }
                }
                Ok(lines)
            }
            "response.completed" => {
                let stop_reason = if self.state.saw_tool_use {
                    "tool_use"
                } else {
                    "end_turn"
                };
                Ok(finalize_anthropic_stream(
                    &mut self.state,
                    Some(stop_reason),
                ))
            }
            "response.failed" | "error" => {
                let message = event
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| event.get("message").and_then(Value::as_str))
                    .unwrap_or("upstream reported stream failure");
                Err(message.to_string())
            }
            _ => Ok(Vec::new()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantTextHistoryMode {
    NativeRole,
    TranscriptUser,
}

fn messages_to_responses_input(
    messages: &[ClaudeMessage],
    assistant_text_mode: AssistantTextHistoryMode,
) -> Result<Vec<Value>, AppError> {
    let mut items = Vec::new();
    for message in messages {
        items.extend(message_to_responses_items(message, assistant_text_mode)?);
    }
    Ok(items)
}

fn message_to_responses_items(
    message: &ClaudeMessage,
    assistant_text_mode: AssistantTextHistoryMode,
) -> Result<Vec<Value>, AppError> {
    let mut items = Vec::new();
    let mut text_parts = Vec::new();
    let mut text_fragments = Vec::new();

    for block in &message.content {
        match block {
            ClaudeContentBlock::Text(block) => {
                text_fragments.push(block.text.to_string());
                text_parts.push(json!({
                    "type": "input_text",
                    "text": block.text
                }));
            }
            ClaudeContentBlock::ToolUse(tool_use) => {
                items.push(tool_use_to_responses_item(tool_use)?);
            }
            ClaudeContentBlock::ToolResult(tool_result) => {
                items.push(tool_result_to_responses_item(tool_result)?);
            }
        }
    }

    if !text_parts.is_empty() {
        items.insert(
            0,
            responses_message_item(
                message.role,
                if message.role == ClaudeRole::Assistant
                    && assistant_text_mode == AssistantTextHistoryMode::TranscriptUser
                {
                    vec![json!({
                        "type": "input_text",
                        "text": transcript_safe_message_text(
                            message.role,
                            text_fragments,
                            assistant_text_mode,
                        )
                    })]
                } else {
                    text_parts
                },
                assistant_text_mode,
            ),
        );
    }

    Ok(items)
}

fn tool_use_to_responses_item(tool_use: &ClaudeToolUse) -> Result<Value, AppError> {
    Ok(json!({
        "type": "function_call",
        "call_id": tool_use.id,
        "name": tool_use.name,
        "arguments": serde_json::to_string(&tool_use.input).map_err(|error| {
            AppError::Internal(format!("failed to serialize anthropic tool_use input: {error}"))
        })?
    }))
}

fn tool_result_to_responses_item(tool_result: &ClaudeToolResult) -> Result<Value, AppError> {
    Ok(json!({
        "type": "function_call_output",
        "call_id": tool_result.tool_use_id,
        "output": tool_result_content_to_string(&tool_result.content)?
    }))
}

fn tool_result_content_to_string(content: &ClaudeToolResultContent) -> Result<String, AppError> {
    match content {
        ClaudeToolResultContent::Empty => Ok(String::new()),
        ClaudeToolResultContent::Text(text) => Ok(text.to_string()),
        ClaudeToolResultContent::TextBlocks(blocks) => Ok(blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")),
        ClaudeToolResultContent::Json(value) => serde_json::to_string(value).map_err(|error| {
            AppError::Internal(format!(
                "failed to serialize anthropic tool_result content: {error}"
            ))
        }),
    }
}

fn responses_message_item(
    role: ClaudeRole,
    content: Vec<Value>,
    assistant_text_mode: AssistantTextHistoryMode,
) -> Value {
    let mapped_role = if role == ClaudeRole::Assistant
        && assistant_text_mode == AssistantTextHistoryMode::TranscriptUser
    {
        "user"
    } else {
        match role {
            ClaudeRole::User => "user",
            ClaudeRole::Assistant => "assistant",
        }
    };
    json!({
        "type": "message",
        "role": mapped_role,
        "content": content
    })
}

fn transcript_safe_message_text(
    role: ClaudeRole,
    text_fragments: Vec<String>,
    assistant_text_mode: AssistantTextHistoryMode,
) -> String {
    let joined = text_fragments.join("\n");
    if role == ClaudeRole::Assistant
        && assistant_text_mode == AssistantTextHistoryMode::TranscriptUser
    {
        format!("Assistant: {joined}")
    } else {
        joined
    }
}

fn map_tool_definition(tool: &ClaudeToolDefinition) -> Result<Value, AppError> {
    let mut mapped = Map::new();
    mapped.insert("type".to_string(), Value::String("function".to_string()));
    mapped.insert("name".to_string(), Value::String(tool.name.to_string()));
    if let Some(description) = &tool.description {
        mapped.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(schema) = &tool.input_schema {
        mapped.insert("parameters".to_string(), schema.clone());
    }
    Ok(Value::Object(mapped))
}

fn map_tool_choice(tool_choice: ClaudeToolChoice) -> Result<Value, AppError> {
    match tool_choice {
        ClaudeToolChoice::Auto => Ok(Value::String("auto".to_string())),
        ClaudeToolChoice::Any => Ok(Value::String("required".to_string())),
        ClaudeToolChoice::Tool { name } => Ok(json!({
            "type": "function",
            "name": name
        })),
    }
}

fn extract_response_output_text(response: &Value) -> String {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return text.to_string();
    }

    let mut parts = Vec::new();
    if let Some(outputs) = response.get("output").and_then(Value::as_array) {
        for output in outputs {
            if let Some(content_items) = output.get("content").and_then(Value::as_array) {
                for item in content_items {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                        continue;
                    }
                    if let Some(text) = item.get("output_text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
            }
        }
    }
    parts.join("")
}

fn extract_response_tool_calls(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(response_output_item_to_chat_tool_call)
        .collect()
}

fn response_output_item_to_chat_tool_call(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }

    let name = item.get("name").and_then(Value::as_str)?;
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = item
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))?;

    Some(json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    }))
}

fn extract_sse_data(frame: &str) -> Option<String> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    (!data.is_empty()).then_some(data)
}

fn ensure_anthropic_message_start(
    requested_model: &str,
    request_id: &str,
    stream_state: &mut AnthropicStreamState,
) -> Vec<String> {
    if stream_state.sent_message_start {
        return Vec::new();
    }
    stream_state.sent_message_start = true;
    vec![anthropic_event_line(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": format!("msg_{request_id}"),
                "type": "message",
                "role": "assistant",
                "model": requested_model,
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }
        }),
    )]
}

fn ensure_anthropic_text_block(
    stream_state: &mut AnthropicStreamState,
    lines: &mut Vec<String>,
) -> usize {
    if let Some(index) = stream_state.text_block_index {
        return index;
    }
    let index = stream_state.next_block_index;
    stream_state.next_block_index += 1;
    stream_state.text_block_index = Some(index);
    stream_state.text_block_closed = false;
    lines.push(anthropic_content_block_start_line(
        index,
        json!({ "type": "text", "text": "" }),
    ));
    index
}

fn close_anthropic_text_block(stream_state: &mut AnthropicStreamState, lines: &mut Vec<String>) {
    if let Some(index) = stream_state.text_block_index {
        if !stream_state.text_block_closed {
            lines.push(anthropic_content_block_stop_line(index));
            stream_state.text_block_closed = true;
        }
    }
}

fn anthropic_tool_block_state<'a>(
    output_index: i64,
    call_id: String,
    name: String,
    stream_state: &'a mut AnthropicStreamState,
) -> &'a mut AnthropicToolBlockState {
    stream_state
        .tool_blocks
        .entry(output_index)
        .or_insert_with(|| {
            let block_index = stream_state.next_block_index;
            stream_state.next_block_index += 1;
            AnthropicToolBlockState {
                block_index,
                call_id,
                name,
                started: false,
                closed: false,
            }
        })
}

fn finalize_anthropic_stream(
    stream_state: &mut AnthropicStreamState,
    stop_reason: Option<&str>,
) -> Vec<String> {
    if stream_state.finalized {
        return Vec::new();
    }

    let mut lines = Vec::new();
    close_anthropic_text_block(stream_state, &mut lines);
    let mut indices = stream_state.tool_blocks.keys().copied().collect::<Vec<_>>();
    indices.sort_unstable();
    for index in indices {
        if let Some(block) = stream_state.tool_blocks.get_mut(&index) {
            if block.started && !block.closed {
                lines.push(anthropic_content_block_stop_line(block.block_index));
                block.closed = true;
            }
        }
    }
    if let Some(stop_reason) = stop_reason {
        lines.push(anthropic_event_line(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": stop_reason,
                    "stop_sequence": Value::Null
                },
                "usage": { "output_tokens": 0 }
            }),
        ));
    }
    lines.push(anthropic_event_line(
        "message_stop",
        json!({ "type": "message_stop" }),
    ));
    stream_state.finalized = true;
    lines
}

fn anthropic_content_block_start_line(index: usize, content_block: Value) -> String {
    anthropic_event_line(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": content_block
        }),
    )
}

fn anthropic_content_block_delta_line(index: usize, delta: Value) -> String {
    anthropic_event_line(
        "content_block_delta",
        json!({
            "type": "content_block_delta",
            "index": index,
            "delta": delta
        }),
    )
}

fn anthropic_content_block_stop_line(index: usize) -> String {
    anthropic_event_line(
        "content_block_stop",
        json!({
            "type": "content_block_stop",
            "index": index
        }),
    )
}

fn anthropic_event_line(event_name: &str, payload: Value) -> String {
    format!(
        "event: {event_name}\ndata: {}\n\n",
        serde_json::to_string(&payload).unwrap()
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::claude::provider_capability_profile::CapabilityDisposition;
    use crate::claude::semantic_core::ClaudeMessageRequest;

    use super::*;

    #[test]
    fn claude_responses_adapter_matches_claude_code_tool_cycle_payload_golden() {
        let request =
            parse_fixture_request(include_str!("fixtures/claude_code_tool_cycle_request.json"));

        let payload = ResponsesProviderAdapter::new()
            .request_to_payload(&request)
            .expect("request should map");
        let expected_payload: Value = serde_json::from_str(include_str!(
            "fixtures/claude_code_tool_cycle_responses_payload.json"
        ))
        .expect("expected payload fixture should parse");
        let policy = ResponsesProviderAdapter::new().extension_policy(&request);

        assert_eq!(payload, expected_payload);
        assert_eq!(policy.service_tier.forwarded.as_deref(), Some("priority"));
        assert_eq!(policy.requested_betas.len(), 2);
        assert_eq!(policy.unsupported_beta_hints.len(), 2);
        assert_eq!(
            policy.thinking.disposition,
            CapabilityDisposition::IgnoreRequested
        );
        assert_eq!(
            policy.context_management.disposition,
            CapabilityDisposition::IgnoreRequested
        );
    }

    #[test]
    fn claude_responses_adapter_exposes_assistant_history_compat_mode() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [
                { "role": "user", "content": "hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "pong" }] }
            ]
        }))
        .unwrap();

        let payload = ResponsesProviderAdapter::new()
            .with_request_mode(ResponsesRequestMode::AssistantHistoryCompat)
            .request_to_payload(&request)
            .expect("compat request should map");

        assert_eq!(payload["input"][1]["role"], "user");
        assert_eq!(payload["input"][1]["content"][0]["text"], "Assistant: pong");
    }

    #[test]
    fn claude_responses_adapter_only_allows_assistant_history_retry_for_compat_profile() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [
                { "role": "user", "content": "hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "pong" }] }
            ]
        }))
        .unwrap();

        let strict_adapter = ResponsesProviderAdapter::new()
            .with_capability_profile(ClaudeProviderCapabilityProfile::responses_strict());
        let compat_adapter = ResponsesProviderAdapter::new()
            .with_capability_profile(ClaudeProviderCapabilityProfile::responses_compat());

        assert!(!strict_adapter.should_retry_with_assistant_history_compat(&request));
        assert!(compat_adapter.should_retry_with_assistant_history_compat(&request));
    }

    #[test]
    fn claude_responses_adapter_matches_interruption_tool_result_payload_golden() {
        let request = parse_fixture_request(include_str!(
            "fixtures/claude_code_interruption_tool_results_request.json"
        ));

        let payload = ResponsesProviderAdapter::new()
            .request_to_payload(&request)
            .expect("tool_result-only request should map");
        let expected_payload: Value = serde_json::from_str(include_str!(
            "fixtures/claude_code_interruption_responses_payload.json"
        ))
        .expect("expected interruption payload should parse");

        assert_eq!(payload, expected_payload);
    }

    #[test]
    fn claude_responses_adapter_maps_responses_json_back_to_claude_message() {
        let response = ResponsesProviderAdapter::new()
            .response_to_message(
                &json!({
                    "id": "resp_123",
                    "output_text": "hello from upstream",
                    "output": [{
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"Paris\"}"
                    }],
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 7
                    }
                }),
                "claude-opus-4-6",
                "req_123",
            )
            .expect("response should map");

        assert_eq!(response["type"], "message");
        assert_eq!(response["role"], "assistant");
        assert_eq!(response["model"], "claude-opus-4-6");
        assert_eq!(response["content"][0]["type"], "text");
        assert_eq!(response["content"][1]["type"], "tool_use");
        assert_eq!(response["content"][1]["input"]["city"], "Paris");
        assert_eq!(response["stop_reason"], "tool_use");
        assert_eq!(response["usage"]["input_tokens"], 11);
        assert_eq!(response["usage"]["output_tokens"], 7);
    }

    #[test]
    fn claude_responses_adapter_translates_stream_frames_to_anthropic_sse() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_123");

        let created = stream
            .translate_frame("data: {\"type\":\"response.created\"}\n\n")
            .expect("created frame should map");
        assert!(created.join("").contains("event: message_start"));

        let delta = stream
            .translate_frame(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )
            .expect("text delta should map");
        let text = delta.join("");
        assert!(text.contains("event: content_block_start"));
        assert!(text.contains("\"type\":\"text_delta\""));
        assert!(text.contains("\"text\":\"Hello\""));

        let tool = stream
            .translate_frame("data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"lookup_weather\",\"arguments\":\"{\\\"city\\\":\\\"Par\"}}\n\n")
            .expect("tool call should map");
        let tool_text = tool.join("");
        assert!(tool_text.contains("\"type\":\"tool_use\""));
        assert!(tool_text.contains("\"partial_json\""));
        assert!(tool_text.contains("call_1"));

        let done = stream
            .translate_frame("data: {\"type\":\"response.completed\"}\n\n")
            .expect("completion frame should map");
        let done_text = done.join("");
        assert!(done_text.contains("event: message_delta"));
        assert!(done_text.contains("\"stop_reason\":\"tool_use\""));
        assert!(done_text.contains("event: message_stop"));
    }

    #[test]
    fn claude_responses_adapter_matches_claude_code_stream_golden() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_fixture");
        let actual = translate_fixture_stream(
            &mut stream,
            include_str!("fixtures/claude_code_responses_stream_tool_cycle.sse"),
        )
        .expect("fixture stream should translate");
        let expected = include_str!("fixtures/claude_code_anthropic_stream_tool_cycle.sse");

        assert_eq!(actual, expected);
    }

    #[test]
    fn claude_responses_adapter_surfaces_stream_failure_for_client_side_error_synthesis() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_123");

        let _ = stream
            .translate_frame("data: {\"type\":\"response.created\"}\n\n")
            .expect("created frame should map");
        let _ = stream
            .translate_frame("data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"lookup_weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}\n\n")
            .expect("tool call should map");

        let failed = stream
            .translate_frame(
                "data: {\"type\":\"response.failed\",\"error\":{\"message\":\"model runtime failed\"}}\n\n",
            )
            .expect_err("failed event should surface an error");
        assert_eq!(failed, "model runtime failed");

        let mut top_level_error = adapter.stream_event_adapter("claude-sonnet-4-6", "req_456");
        let error = top_level_error
            .translate_frame("data: {\"type\":\"error\",\"message\":\"gateway exploded\"}\n\n")
            .expect_err("top-level error event should surface an error");
        assert_eq!(error, "gateway exploded");

        let mut fixture_stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_fixture");
        let fixture_error = translate_fixture_stream(
            &mut fixture_stream,
            include_str!("fixtures/claude_code_responses_stream_failure.sse"),
        )
        .expect_err("failure fixture should surface an error");
        assert_eq!(fixture_error, "model runtime failed");
    }

    #[test]
    fn claude_responses_adapter_done_frame_synthesizes_terminal_message_delta_once() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_123");

        let _ = stream
            .translate_frame("data: {\"type\":\"response.created\"}\n\n")
            .expect("created frame should map");
        let _ = stream
            .translate_frame(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )
            .expect("text delta should map");

        let done_only = stream
            .translate_frame("data: [DONE]\n\n")
            .expect("done frame should map");
        let done_only_text = done_only.join("");
        assert!(done_only_text.contains("event: content_block_stop"));
        assert!(done_only_text.contains("event: message_delta"));
        assert!(done_only_text.contains("\"stop_reason\":\"end_turn\""));
        assert!(done_only_text.contains("event: message_stop"));

        let duplicate_done = stream
            .translate_frame("data: [DONE]\n\n")
            .expect("duplicate done should not fail");
        assert!(duplicate_done.is_empty());
    }

    #[test]
    fn claude_responses_adapter_preserves_claude_stream_event_order_assumed_by_query_engine() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_123");

        let _ = stream
            .translate_frame("data: {\"type\":\"response.created\"}\n\n")
            .expect("created frame should map");
        let _ = stream
            .translate_frame(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )
            .expect("text delta should map");

        let terminal = stream
            .translate_frame("data: {\"type\":\"response.completed\"}\n\n")
            .expect("completion frame should map")
            .join("");

        let stop_index = terminal
            .find("event: content_block_stop")
            .expect("text block stop should be present");
        let delta_index = terminal
            .find("event: message_delta")
            .expect("message_delta should be present");
        let message_stop_index = terminal
            .find("event: message_stop")
            .expect("message_stop should be present");

        assert!(stop_index < delta_index);
        assert!(delta_index < message_stop_index);
        assert!(terminal.contains("\"stop_reason\":\"end_turn\""));
    }

    #[test]
    fn claude_responses_adapter_closes_text_block_before_tool_use_block() {
        let adapter = ResponsesProviderAdapter::new();
        let mut stream = adapter.stream_event_adapter("claude-sonnet-4-6", "req_123");

        let _ = stream
            .translate_frame("data: {\"type\":\"response.created\"}\n\n")
            .expect("created frame should map");
        let _ = stream
            .translate_frame(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )
            .expect("text delta should map");

        let tool_transition = stream
            .translate_frame("data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"lookup_weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}\n\n")
            .expect("tool call should map")
            .join("");

        let text_stop_index = tool_transition
            .find("event: content_block_stop")
            .expect("text block should close first");
        let tool_start_index = tool_transition[text_stop_index + 1..]
            .find("\"type\":\"tool_use\"")
            .map(|index| index + text_stop_index + 1)
            .expect("tool use start should follow");

        assert!(text_stop_index < tool_start_index);
        assert!(tool_transition.contains("\"partial_json\":\"{\\\"city\\\":\\\"Paris\\\"}\""));
    }

    fn parse_fixture_request(raw: &str) -> ClaudeMessageRequest {
        let value: Value = serde_json::from_str(raw).expect("fixture request should parse");
        ClaudeMessageRequest::parse_json(&value).expect("fixture request should be valid")
    }

    fn translate_fixture_stream(
        stream: &mut AnthropicStreamEventAdapter,
        fixture: &str,
    ) -> Result<String, String> {
        let mut output = String::new();
        for frame in fixture
            .split("\n\n")
            .filter(|frame| !frame.trim().is_empty())
            .map(|frame| format!("{frame}\n\n"))
        {
            let lines = stream.translate_frame(&frame)?;
            for line in lines {
                output.push_str(&line);
            }
        }
        Ok(output)
    }
}
