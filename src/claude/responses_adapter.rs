use serde_json::Value;

use crate::{
    claude::semantic_core::ClaudeMessageRequest,
    error::AppError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponsesRequestMode {
    #[default]
    Standard,
    AssistantHistoryCompat,
}

#[derive(Debug, Clone, Default)]
pub struct ResponsesProviderAdapter {
    request_mode: ResponsesRequestMode,
}

pub struct AnthropicStreamEventAdapter;

impl ResponsesProviderAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_request_mode(mut self, request_mode: ResponsesRequestMode) -> Self {
        self.request_mode = request_mode;
        self
    }

    pub fn request_mode(&self) -> ResponsesRequestMode {
        self.request_mode
    }

    pub fn should_retry_with_assistant_history_compat(
        &self,
        request: &ClaudeMessageRequest,
    ) -> bool {
        request.has_plaintext_assistant_history()
    }

    pub fn request_to_payload(&self, _request: &ClaudeMessageRequest) -> Result<Value, AppError> {
        Err(AppError::BadRequest(
            "claude responses adapter request mapping not implemented".to_string(),
        ))
    }

    pub fn response_to_message(
        &self,
        _response: &Value,
        _requested_model: &str,
        _request_id: &str,
    ) -> Result<Value, AppError> {
        Err(AppError::BadRequest(
            "claude responses adapter response mapping not implemented".to_string(),
        ))
    }

    pub fn stream_event_adapter(
        &self,
        _requested_model: impl Into<String>,
        _request_id: impl Into<String>,
    ) -> AnthropicStreamEventAdapter {
        AnthropicStreamEventAdapter
    }
}

impl AnthropicStreamEventAdapter {
    pub fn translate_frame(&mut self, _frame: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::claude::semantic_core::ClaudeMessageRequest;

    use super::*;

    #[test]
    fn claude_responses_adapter_maps_semantic_request_to_responses_payload() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "system": [{ "type": "text", "text": "be concise" }],
            "stream": true,
            "max_tokens": 64,
            "temperature": 0.1,
            "top_p": 0.8,
            "tool_choice": { "type": "tool", "name": "lookup_weather" },
            "tools": [{
                "name": "lookup_weather",
                "description": "Fetch weather",
                "input_schema": { "type": "object" }
            }],
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "weather?" }] },
                { "role": "assistant", "content": [{ "type": "text", "text": "calling tool" }] },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "lookup_weather",
                        "input": { "city": "Paris" }
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": [{ "type": "text", "text": "sunny" }]
                    }]
                }
            ]
        }))
        .unwrap();

        let payload = ResponsesProviderAdapter::new()
            .request_to_payload(&request)
            .expect("request should map");

        assert_eq!(payload["model"], "claude-sonnet-4-6");
        assert_eq!(payload["instructions"], "be concise");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["max_output_tokens"], 64);
        assert_eq!(payload["tool_choice"]["name"], "lookup_weather");
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["input"][0]["type"], "message");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][1]["role"], "assistant");
        assert_eq!(payload["input"][2]["type"], "function_call");
        assert_eq!(payload["input"][3]["type"], "function_call_output");
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
            .translate_frame("data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n")
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
        assert!(tool_text.contains("\"partial_json\":\"{\\\\\\\"city\\\\\\\":\\\\\\\"Par\""));

        let done = stream
            .translate_frame("data: {\"type\":\"response.completed\"}\n\n")
            .expect("completion frame should map");
        let done_text = done.join("");
        assert!(done_text.contains("event: message_delta"));
        assert!(done_text.contains("\"stop_reason\":\"tool_use\""));
        assert!(done_text.contains("event: message_stop"));
    }
}
