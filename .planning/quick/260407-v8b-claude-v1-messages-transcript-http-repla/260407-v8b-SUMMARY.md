# Quick Task 260407-v8b Summary

## Task

补齐 Claude `/v1/messages` 还没被 HTTP golden replay 锁住的必做兼容：多轮 transcript、stream chunk-boundary failure 回放，以及 Anthropic request-id/error/retry-header fidelity。

## Result

- 新增 follow-up transcript fixtures，补上 Claude Code 第二轮只回 `tool_result` 的 HTTP golden：
  - `src/claude/fixtures/claude_code_http_transcript_followup_request.json`
  - `src/claude/fixtures/claude_code_http_transcript_followup_responses_payload.json`
  - `src/claude/fixtures/claude_code_http_transcript_followup_message.json`
- 新增 stream failure fixtures，把 mid-stream failure 前已成功下发的 Anthropic SSE 前缀锁成 golden：
  - `src/claude/fixtures/claude_code_http_stream_failure_request.json`
  - `src/claude/fixtures/claude_code_http_stream_failure_prefix.sse`
- `src/http.rs` 的 replay harness 现在支持：
  - 带自定义 headers 的 replay response
  - 按指定 chunk boundary 回放 SSE/body
  - 从 chunked failure stream 中拆出“已成功发出的前缀”和最终错误
- `src/http.rs` 新增 3 条必做回归测试：
  - `claude_http_golden_replay_transcript_followup`
  - `claude_http_golden_replay_stream_failure_chunked`
  - `claude_messages_propagates_anthropic_error_headers`
- `/v1/messages` 的 upstream error fidelity 现在在两条路径都能保真：
  - HTTP proxy 直接返回 Anthropic error envelope 时，会保留 upstream `request-id`、`retry-after`、`x-should-retry` 和原始 error type/message
  - 通用 `AppError -> Anthropic error` 路径也会带上这些 upstream 元数据，避免边角错误路径丢 header
- 这批回归继续锁定单一 upstream core：
  - 下游仍然只测 `/v1/messages`
  - route log 和 captured upstream payload 仍然只命中 `/v1/responses`

## Notes

- follow-up transcript fixture 刻意收敛为最小必要输入，只保留 Claude Code 第二轮真正关键的 `tool_result -> function_call_output` 语义，避免把第一轮可复用字段重复固化两遍。
- stream failure replay 不是简单 `contains` 断言，而是验证 chunk 边界打散后，客户端在报错前已经收到的 Anthropic SSE 前缀仍然与 golden 完全一致。

## Verification

- `cargo fmt --all`
- `cargo test claude_http_golden_replay_transcript_followup --lib`
- `cargo test claude_http_golden_replay_stream_failure_chunked --lib`
- `cargo test claude_messages_propagates_anthropic_error_headers --lib`
- `cargo test claude_native_gateway_skeleton --lib`
- `cargo test --lib`

## Commits

- `386ce4f` `feat(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): add claude transcript replay fixtures`
- `d7f0315` `test(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): extend claude http replay coverage`
- `9d93aef` `fix(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): preserve claude message error fidelity`
- `f9abb6c` `fix(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): thread upstream metadata through anthropic errors`
