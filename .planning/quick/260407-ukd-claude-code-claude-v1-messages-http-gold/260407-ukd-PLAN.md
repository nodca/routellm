---
phase: quick-260407-ukd-claude-code-claude-v1-messages-http-gold
plan: 01
type: execute
wave: 1
depends_on:
  - quick-260407-u8c
files_modified:
  - src/claude/fixtures/claude_code_http_nonstream_request.json
  - src/claude/fixtures/claude_code_http_nonstream_message.json
  - src/claude/fixtures/claude_code_http_stream_request.json
  - src/claude/fixtures/claude_code_http_stream_anthropic.sse
  - src/claude/fixtures/claude_code_http_compat_request.json
  - src/claude/fixtures/claude_code_http_compat_first_responses_payload.json
  - src/claude/fixtures/claude_code_http_compat_second_responses_payload.json
  - src/claude/fixtures/claude_code_http_compat_message.json
  - src/http.rs
autonomous: true
requirements:
  - quick-260407
user_setup: []
must_haves:
  truths:
    - "Claude `/v1/messages` 的 HTTP 兼容边界有 fixture-driven golden replay tests，而不是继续停留在骨架级 contains 断言。"
    - "non-stream、stream、compat fallback 三条路径都以 Claude Code 样本为基准，验证下游看到的消息形态和上游 `/v1/responses` payload。"
    - "仍然只保留一个上游核心 `/v1/responses`，不新增第二套 Claude 专用 provider core。"
  artifacts:
    - path: src/claude/fixtures/claude_code_http_nonstream_message.json
      provides: "non-stream `/v1/messages` downstream golden"
    - path: src/claude/fixtures/claude_code_http_stream_anthropic.sse
      provides: "stream `/v1/messages` downstream SSE golden"
    - path: src/http.rs
      provides: "fixture-backed `/v1/messages` HTTP replay tests and replay upstream helpers"
  key_links:
    - from: src/http.rs
      to: src/claude/fixtures/claude_code_http_nonstream_message.json
      via: "non-stream replay test body equality"
      pattern: "claude_http_golden_replay_nonstream"
    - from: src/http.rs
      to: src/claude/fixtures/claude_code_http_stream_anthropic.sse
      via: "stream replay test SSE equality"
      pattern: "claude_http_golden_replay_stream"
    - from: src/http.rs
      to: src/claude/fixtures/claude_code_http_compat_second_responses_payload.json
      via: "compat retry captured second upstream payload equality"
      pattern: "claude_http_golden_replay_compat_retry"
---

<objective>
基于 Claude Code 源码补充 Claude `/v1/messages` HTTP 端到端 golden replay tests，覆盖 non-stream、stream 与 compat fallback 路径。

Purpose: 把上一轮已经沉淀在 `responses_adapter` 层的 source-driven fixtures 再往上抬一层，直接锁定 HTTP compatibility boundary 的真实回放行为，避免后续只靠零散 contains 断言看起来“差不多能用”。
Output: 一个原子的 quick plan，新增 HTTP replay fixture/golden 资产，并在 `src/http.rs` 加上基于 `/v1/responses` 的端到端 golden replay tests。
</objective>

<execution_context>
Relevant context:
- `.planning/STATE.md`
- `.planning/quick/260407-u8c-claude-code-claude-messages-request-stre/260407-u8c-SUMMARY.md`
- `src/http.rs`
- `src/claude/responses_adapter.rs`
- `src/claude/provider_capability_profile.rs`
- `src/claude/fixtures/*`

Scope guard:
- 只围绕 Claude `/v1/messages` HTTP compatibility boundary 补强
- 上游核心继续只有 `/v1/responses`
- 不新增 schema / route / storage 设计
- 优先复用现有 Claude Code source-driven request/stream fixtures，不重造 fixture 体系
</execution_context>

<context>
<interfaces>
From `src/http.rs`:

```rust
fn build_claude_message_payloads(payload: Value) -> Result<PreparedPayloads, AppError>
```

```rust
fn claude_response_adapter(
    &self,
    capability_profile: &ClaudeProviderCapabilityProfile,
) -> Option<ResponsesProviderAdapter>
```

Compat retry already exists and should be verified rather than redesigned:

```rust
if status.is_server_error()
    && dispatch.payload_kind == DispatchPayloadKind::AnthropicMessagesToResponses
    && claude_capability_profile
        .as_ref()
        .is_some_and(|profile| payloads.should_retry_with_assistant_history_compat(profile))
{
    // retry with AnthropicMessagesToResponsesAssistantHistoryCompat
}
```

From `src/claude/provider_capability_profile.rs`:

```rust
pub fn responses_strict() -> Self
pub fn responses_compat() -> Self
pub fn supports_assistant_history_compat_retry(&self) -> bool
```

Use these existing boundaries directly. Do not introduce a second provider path or a Claude-only transport branch.
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add HTTP replay fixtures and downstream goldens</name>
  <files>src/claude/fixtures/claude_code_http_nonstream_request.json, src/claude/fixtures/claude_code_http_nonstream_message.json, src/claude/fixtures/claude_code_http_stream_request.json, src/claude/fixtures/claude_code_http_stream_anthropic.sse, src/claude/fixtures/claude_code_http_compat_request.json, src/claude/fixtures/claude_code_http_compat_first_responses_payload.json, src/claude/fixtures/claude_code_http_compat_second_responses_payload.json, src/claude/fixtures/claude_code_http_compat_message.json</files>
  <action>基于现有 `claude_code_tool_cycle_request.json`、`claude_code_responses_stream_tool_cycle.sse`、`claude_code_interruption_*` fixture 和 Claude Code 源码依赖的消息语义，新增 HTTP 层 replay 资产。non-stream/stream fixture 要表达“下游发 `/v1/messages`，上游仍走 `/v1/responses`，再回到 Anthropic message/SSE”的完整样本；compat fixture 要额外固化第一次 strict payload 与第二次 assistant-history compat payload。避免重新发明命名体系或复制内联测试样本，优先抽出当前 `http.rs` skeleton tests 已经隐含的真实请求/响应形态。</action>
  <verify>
    <automated>cargo test claude_http_golden_replay_nonstream --lib</automated>
  </verify>
  <done>fixture 目录里新增三条 HTTP replay 场景的请求/响应 golden，且 compat 场景同时具备 strict 与 compat 两份上游 payload 基准。</done>
</task>

<task type="auto">
  <name>Task 2: Add fixture-backed non-stream and stream `/v1/messages` replay tests</name>
  <files>src/http.rs</files>
  <action>在 `src/http.rs` 的测试模块中增加一个可复用的 fixture replay upstream helper：它只暴露 `/v1/responses`，按 fixture 返回 JSON 或 SSE，并记录收到的请求体。然后把当前 `claude_native_gateway_skeleton_keeps_nonstream_messages_flow` 与 `claude_native_gateway_skeleton_keeps_stream_tool_flow` 提升为真正的 golden replay tests：用 HTTP request fixture 发起 `/v1/messages`，断言捕获到的上游 payload 与 golden 一致，断言下游 non-stream body / stream SSE 在稳定字段归一化后与 golden 完全一致，而不是只做 `contains`。保留现有适配路径，不新增 transport 分支。</action>
  <verify>
    <automated>cargo test claude_http_golden_replay_nonstream --lib && cargo test claude_http_golden_replay_stream --lib</automated>
  </verify>
  <done>non-stream 与 stream 两条 `/v1/messages` HTTP replay tests 都通过 fixture/golden 做精确回归，且确认上游请求仍然命中 `/v1/responses`。</done>
</task>

<task type="auto">
  <name>Task 3: Lock compat fallback with end-to-end replay and gateway assertions</name>
  <files>src/http.rs</files>
  <action>把当前 `claude_messages_retry_with_assistant_history_compat_after_upstream_5xx` 收敛成 fixture-backed compat fallback replay test。测试必须模拟 compat profile 的 `/v1/responses` 上游第一次返回 5xx、第二次成功，校验两次捕获的 payload 分别匹配 strict/compat goldens，最终 `/v1/messages` 下游消息匹配 golden，并继续验证 request log 里的 `gateway.profile=compat-responses`、`assistant_history.compat_retry_allowed=true`、`fallback_applied=true`、`fallback_trigger_status=502`。如果现有测试 helper 不足，优先抽小型通用 helper，不要再加一套临时 harness。</action>
  <verify>
    <automated>cargo test claude_http_golden_replay_compat_retry --lib && cargo test claude_native_gateway_skeleton --lib</automated>
  </verify>
  <done>compat fallback 行为被端到端 golden 回放锁定：第一次 strict、第二次 compat、最终 Anthropic message 正确，且日志字段和 fallback 决策一致。</done>
</task>

</tasks>

<verification>
- `cargo test claude_http_golden_replay_nonstream --lib`
- `cargo test claude_http_golden_replay_stream --lib`
- `cargo test claude_http_golden_replay_compat_retry --lib`
- `cargo test claude_native_gateway_skeleton --lib`
- `cargo fmt`
- `cargo test --lib`
</verification>

<success_criteria>
- `src/http.rs` 里出现 3 个基于 fixture/golden 的 Claude `/v1/messages` HTTP replay tests，分别覆盖 non-stream、stream、compat fallback。
- 每条 replay test 都明确验证“下游 `/v1/messages` -> 上游 `/v1/responses` -> 下游 Anthropic shape”这条单核路径。
- stream 与 non-stream 的断言从骨架级 contains 升级为 golden compare；compat fallback 同时校验上下游结果和 gateway fallback 记录。
</success_criteria>

<output>
After completion, update `.planning/quick/260407-ukd-claude-code-claude-v1-messages-http-gold/260407-ukd-SUMMARY.md`
</output>
