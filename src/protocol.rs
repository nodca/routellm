use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Responses,
    ChatCompletions,
    Messages,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::Responses => "/v1/responses",
            Self::ChatCompletions => "/v1/chat/completions",
            Self::Messages => "/v1/messages",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim() {
            "responses" => Ok(Self::Responses),
            "chat_completions" => Ok(Self::ChatCompletions),
            "claude" | "messages" => Ok(Self::Messages),
            other => Err(AppError::BadRequest(format!(
                "field `protocol` must be one of responses, chat_completions, messages; got `{other}`"
            ))),
        }
    }
}

pub fn compatibility_cost(channel_protocol: Protocol, request_protocol: Protocol) -> Option<u8> {
    match (request_protocol, channel_protocol) {
        (Protocol::Responses, Protocol::Responses) => Some(0),
        (Protocol::Messages, Protocol::Messages) => Some(0),
        (Protocol::ChatCompletions, Protocol::ChatCompletions) => Some(0),
        (Protocol::ChatCompletions, Protocol::Responses) => Some(1),
        _ => None,
    }
}
