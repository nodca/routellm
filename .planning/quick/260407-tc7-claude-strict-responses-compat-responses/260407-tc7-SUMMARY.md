# Quick Task 260407-tc7 Summary

## Task

为 Claude 原生网关增加 strict responses / compat responses capability profiles，并将 fallback 与降级日志绑定到 profile 决策。

## Result

- 在 [provider_capability_profile.rs](/home/wcn/metapi-rs/src/claude/provider_capability_profile.rs) 新增了 `strict-responses` / `compat-responses` 两套 profile。
- 通过 `for_responses_endpoint()` 按 channel `base_url` 区分官方 OpenAI Responses 上游和兼容 Responses 上游：
  - `api.openai.com` -> `strict-responses`
  - 其他 Responses 端点 -> `compat-responses`
- [responses_adapter.rs](/home/wcn/metapi-rs/src/claude/responses_adapter.rs) 的 assistant-history compat retry 现在不再只看消息形态，而是同时受 profile 约束：
  - `strict-responses` 不自动降级
  - `compat-responses` 允许在存在 plaintext assistant history 时触发 compat retry
- [http.rs](/home/wcn/metapi-rs/src/http.rs) 现在会：
  - 按 selected channel 选择 Claude profile
  - 按 `profile + request_mode` 缓存 Claude->Responses payload
  - 在现有 `claude_request_fingerprint.gateway` 中记录：
    - 使用的 profile
    - assistant-history 是否允许 compat retry
    - fallback 是否实际触发
    - fallback 触发状态码
    - Claude 扩展字段的 disposition 摘要
- 现有行为保持住了：
  - Claude `/v1/messages` 仍只走一个 `/v1/responses` upstream core
  - official / strict path 不会被 compat 逻辑污染
  - compat path 在出现第二轮消息 5xx 时能自动切 assistant-history compat payload
  - request logs 里现在能看出这次请求到底用了什么 Claude gateway 决策

## Verification

- `cargo fmt`
- `cargo test claude_provider_capability_profile --lib`
- `cargo test claude_responses_adapter --lib`
- `cargo test claude_native_gateway_skeleton --lib`
- `cargo test claude_messages_retry_with_assistant_history_compat_after_upstream_5xx --lib`
- `cargo test claude_code_style_message_failures_log_redacted_fingerprint_and_correlation --lib`
- `cargo test --lib`

## Notes

- 这一步实现的是 “按上游 profile 决定兼容策略”，不是把 Claude 兼容层再扩展成第二套路由核心。
- 对你现在这种 `free.9e.nz` 兼容 Responses 上游，路由会直接归到 `compat-responses`，因此第二条消息命中 5xx 时会自动尝试 assistant-history compat payload。
