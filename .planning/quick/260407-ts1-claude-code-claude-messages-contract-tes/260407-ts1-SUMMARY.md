# Quick Task 260407-ts1 Summary

## Task

基于 Claude Code 源码补充 Claude messages 原生 contract tests，覆盖 message shape、SSE 顺序、tool use 与错误语义。

## Source Findings

从 [QueryEngine.ts](/home/wcn/Downloads/CC-Source/src/QueryEngine.ts)、[query.ts](/home/wcn/Downloads/CC-Source/src/query.ts) 和 [coreSchemas.ts](/home/wcn/Downloads/CC-Source/src/entrypoints/sdk/coreSchemas.ts) 提炼出几个关键契约：

- Claude Code 对 streamed assistant message 的 `stop_reason` 依赖 `message_delta`，而不是 content block stop 时的 message 本体。
- `stop_reason == "tool_use"` 本身不总可靠，源码更依赖“流里确实收到了 tool_use block”这个事实。
- stream 终止时，`content_block_stop -> message_delta -> message_stop` 的顺序很重要。
- 如果 runtime 异常或中断，源码会很在意 tool_use / tool_result 是否成对，避免出现悬空 tool_use。

## Result

- 在 [responses_adapter.rs](/home/wcn/metapi-rs/src/claude/responses_adapter.rs) 增加了 3 个源码驱动 contract tests：
  - `claude_responses_adapter_done_frame_synthesizes_terminal_message_delta_once`
  - `claude_responses_adapter_preserves_claude_stream_event_order_assumed_by_query_engine`
  - `claude_responses_adapter_closes_text_block_before_tool_use_block`
- 顺手修复了一个兼容性边角：
  - 当兼容 Responses 上游只发 `[DONE]` 时，现在会合成终止 `message_delta`
  - 当上游既发 `response.completed` 又发 `[DONE]` 时，不会重复输出 `message_stop`
- 这样对“杂牌 Responses 兼容站”的容错会更好，同时不会影响官方 Responses 路径。

## Verification

- `cargo test claude_responses_adapter --lib`
- `cargo test anthropic_stream_maps_tool_use_events_from_responses_stream --lib`
- `cargo fmt`
- `cargo test --lib`

## Notes

- 这一步主要是在补“Claude Code 真正依赖什么事件语义”，而不是再加一层 fallback。
- 后续如果继续往原生体验逼近，下一块最值得补的是 tool_result / interruption / synthetic error 路径的 contract tests。
