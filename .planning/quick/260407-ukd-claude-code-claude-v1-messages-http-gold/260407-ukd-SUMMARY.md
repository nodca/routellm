# Quick Task 260407-ukd Summary

## Task

基于 Claude Code 源码补充 Claude `/v1/messages` HTTP 端到端 golden replay tests，覆盖 non-stream、stream 与 compat fallback 路径。

## Result

- 新增了一组 HTTP replay fixture，放在 `src/claude/fixtures/claude_code_http_*`：
  - `claude_code_http_nonstream_request.json`
  - `claude_code_http_nonstream_message.json`
  - `claude_code_http_stream_request.json`
  - `claude_code_http_stream_anthropic.sse`
  - `claude_code_http_compat_request.json`
  - `claude_code_http_compat_first_responses_payload.json`
  - `claude_code_http_compat_second_responses_payload.json`
  - `claude_code_http_compat_message.json`
- `src/http.rs` 现在有 3 条 fixture-backed replay tests：
  - `claude_http_golden_replay_nonstream`
  - `claude_http_golden_replay_stream`
  - `claude_http_golden_replay_compat_retry`
- 这三条测试都明确锁定了单 upstream core：
  - 下游请求显式发往 `/v1/messages`
  - 捕获到的上游 payload 全部命中 `/v1/responses`
  - non-stream / stream / compat retry 的下游 Anthropic message/SSE 都与 golden fixture 做精确比对
- compat retry 额外验证了：
  - 第一次 strict payload 命中 `claude_code_http_compat_first_responses_payload.json`
  - 第二次 assistant-history compat payload 命中 `claude_code_http_compat_second_responses_payload.json`
  - request log 中的 `gateway.profile=compat-responses`、`compat_retry_allowed=true`、`fallback_applied=true`、`fallback_trigger_status=502`

## Notes

- route log 对 downstream path 继续记录 canonical protocol path，因此测试把 `/v1/messages` 校验放在实际 request URI 上，把 log 断言聚焦在 upstream `/v1/responses` 与 fallback fingerprint。
- 旧的 compat 专用 stub upstream helper 已移除，改为统一复用 fixture replay upstream。

## Verification

- `cargo fmt`
- `cargo test claude_http_golden_replay_nonstream --lib`
- `cargo test claude_http_golden_replay_stream --lib`
- `cargo test claude_http_golden_replay_compat_retry --lib`
- `cargo test claude_native_gateway_skeleton --lib`
- `cargo test --lib`

## Commits

- `82a11b4` `feat(quick-260407-ukd-claude-code-claude-v1-messages-http-gold-01): add claude messages http replay fixtures`
- `e65ceeb` `test(quick-260407-ukd-claude-code-claude-v1-messages-http-gold-01): add claude messages http replay tests`
