# Quick Task 260407-v8b Summary

## Task

补齐 Claude `/v1/messages` 在 HTTP replay 层缺的三块回归覆盖：多轮 transcript follow-up、chunked stream failure 边界，以及 Claude Code 真实读取的 request-id / retry header / error type 保真。

## Result

- 新增并对齐了 `src/claude/fixtures/claude_code_http_*` 资产：
  - `claude_code_http_transcript_followup_request.json`
  - `claude_code_http_transcript_followup_responses_payload.json`
  - `claude_code_http_transcript_followup_message.json`
  - `claude_code_http_stream_failure_request.json`
  - `claude_code_http_stream_failure_prefix.sse`
- `src/http.rs` 的 replay upstream helper 现在支持：
  - 自定义 response headers
  - 按预设 chunk 边界回放 body
  - 继续复用同一个 `/v1/responses` replay harness，不新增第二套 Claude upstream stub
- 新增了 3 条关键 HTTP 回归测试：
  - `claude_http_golden_replay_transcript_followup`
  - `claude_http_golden_replay_stream_failure_chunked`
  - `claude_messages_propagates_anthropic_error_headers`
- `/v1/messages` 的 upstream error path 现在会保留 Claude Code 依赖的字段：
  - `request-id` 优先取 upstream header，没有时回落到 error body
  - `retry-after` / `x-should-retry` 继续对下游可见
  - Anthropic-style `error.type` 不再被 generic wrapper 冲掉

## Notes

- transcript follow-up golden 最终收敛为 Claude Code 风格的 tool-result-only 第二轮，而不是重复首轮的 system/tool decoration；这是最终通过回归测试并与现有 fixture baseline 对齐的版本。
- full `cargo test --lib` 过程中，`claude_code_style_message_failures_log_redacted_fingerprint_and_correlation` 的旧断言从 generic `api_error` 调整为保留 upstream `server_error`，以匹配这次保真增强后的行为。
- 工作区里仍有一个未提交的现有改动：[src/error.rs](/home/wcn/metapi-rs/src/error.rs)，这次 quick task 没有把它纳入提交。

## Verification

- `cargo test claude_http_golden_replay_transcript_followup --lib`
- `cargo test claude_http_golden_replay_stream_failure_chunked --lib`
- `cargo test claude_messages_propagates_anthropic_error_headers --lib`
- `cargo test claude_native_gateway_skeleton --lib`
- `cargo fmt`
- `cargo test --lib`

## Commits

- `386ce4f` `feat(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): add claude transcript replay fixtures`
- `d7f0315` `test(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): extend claude http replay coverage`
- `9d93aef` `fix(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): preserve claude message error fidelity`
- `fb3328e` `fix(quick-260407-v8b-claude-v1-messages-transcript-http-repla-01): align followup replay goldens`
