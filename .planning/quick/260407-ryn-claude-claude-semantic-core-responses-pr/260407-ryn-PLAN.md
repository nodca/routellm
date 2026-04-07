---
phase: quick-260407-ryn-claude-claude-semantic-core-responses-pr
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/claude/mod.rs
  - src/claude/semantic_core.rs
  - src/claude/responses_adapter.rs
  - src/http.rs
  - src/lib.rs
autonomous: true
requirements:
  - quick-260407
user_setup: []
must_haves:
  truths:
    - "Claude `/v1/messages` handling has an explicit transport-agnostic semantic contract instead of living only as ad-hoc `serde_json::Value` walking inside `src/http.rs`."
    - "The Claude-native path keeps `/v1/responses` as the single upstream provider core, with a named adapter seam that maps Claude semantics to and from Responses payloads and stream events."
    - "Existing message ingress still works through the new core/adapter skeleton for the currently supported slice, so this refactor clarifies architecture without introducing a second proxy stack."
  artifacts:
    - path: src/claude/semantic_core.rs
      provides: "Claude semantic request/content/tool contracts plus minimal parse/normalize helpers"
    - path: src/claude/responses_adapter.rs
      provides: "Thin Responses provider adapter for request mapping, non-stream response mapping, and stream event adaptation"
    - path: src/http.rs
      provides: "HTTP boundary that delegates Claude message semantics to the extracted core/adapter seam"
    - path: src/lib.rs
      provides: "crate exports for the new Claude gateway module boundary"
  key_links:
    - from: src/http.rs
      to: src/claude/semantic_core.rs
      via: "create_message parses a normalized Claude request before dispatch"
      pattern: "create_message"
    - from: src/http.rs
      to: src/claude/responses_adapter.rs
      via: "message ingress uses the adapter for Responses request/response/stream conversion"
      pattern: "ResponsesProviderAdapter"
    - from: src/claude/responses_adapter.rs
      to: src/http.rs
      via: "adapter output feeds the existing Responses dispatch path instead of a parallel protocol core"
      pattern: "Protocol::Responses"
---

<objective>
为 Claude 专用原生网关抽出第一条清晰主链：`semantic_core -> responses_adapter -> existing responses dispatch`。

Purpose: 把 Claude 语义从 `src/http.rs` 巨石里切出一个可继续演进的核心边界，同时坚持项目已锁定的决策: 上游统一收敛到 `/v1/responses`，兼容层保持薄，不再长出第二套路由核心。
Output: 新的 `src/claude/` 模块骨架、最小 HTTP wiring、以及锁定骨架的回归测试。
</objective>

<execution_context>
@/home/wcn/.codex/get-shit-done/workflows/quick.md
@/home/wcn/.codex/get-shit-done/workflows/execute-plan.md
@/home/wcn/.codex/get-shit-done/templates/summary.md

Scope guard for this quick slice:
- 这是架构收束和骨架提取，不是完整 Claude parity 项目。
- 只覆盖仓库已经基本具备的 Claude slice: text messages、基础 tool_use/tool_result、当前 Responses SSE -> Anthropic SSE 映射、以及已有的 assistant-history compatibility fallback。
- 不改路由策略、不改管理 API、不改持久化 schema、不扩展新 provider family。
</execution_context>

<context>
@.planning/STATE.md
@./CLAUDE.md
@.planning/PROJECT.md
@.planning/quick/260407-oo3-claude-code-llmrouter-claude/260407-oo3-SUMMARY.md
@src/lib.rs
@src/app.rs
@src/http.rs
@src/error.rs

Locked project decisions to honor:
- 上游协议统一收敛到 `/v1/responses`
- `chat/completions` 和 `messages` 都是兼容边界，不是第二套路由核心
- 兼容层必须是薄层；如果兼容逻辑开始反向污染核心模型，应优先重构边界

Current local interfaces:
```rust
// src/app.rs
let v1_router = Router::new()
    .route("/responses", axum::routing::post(http::create_response))
    .route("/chat/completions", axum::routing::post(http::create_chat_completion))
    .route("/messages", axum::routing::post(http::create_message));
```

```rust
// src/http.rs
pub async fn create_message(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body>

async fn proxy_request(
    state: AppState,
    requested_model: String,
    payload: Value,
    request_protocol: Protocol,
    request_id: Option<String>,
    log_context: RequestLogContext,
) -> Result<Response<Body>, AppError>
```

```rust
// src/http.rs
fn anthropic_messages_to_responses_payload(payload: &Value) -> Result<Value, AppError>
fn responses_json_to_anthropic_message(
    response: &Value,
    requested_model: &str,
    request_id: &str,
) -> Result<Value, AppError>
fn transform_responses_frame_to_anthropic_sse(
    frame: &str,
    requested_model: &str,
    request_id: &str,
    stream_state: &mut AnthropicStreamState,
) -> Result<Vec<String>, String>
```

Recent baseline from `260407-oo3`:
- `/v1/messages` already has Anthropic-style error envelopes and request-id correlation.
- Claude request fingerprints are already logged; preserve that behavior while extracting architecture.
- The next step is structural: make Claude semantics a first-class module instead of a cluster of `http.rs` helpers.
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Define the Claude semantic core contracts before moving provider logic</name>
  <files>src/claude/mod.rs, src/claude/semantic_core.rs, src/lib.rs</files>
  <read_first>src/lib.rs, src/http.rs, src/error.rs</read_first>
  <behavior>
    - A minimal Claude messages payload with `model`, `messages`, optional `system`, `stream`, `max_tokens`, `tools`, and `tool_choice` can be parsed into a typed semantic request without any Axum or Reqwest types.
    - User/assistant text blocks plus `tool_use` and `tool_result` are represented explicitly as Rust enums/structs instead of anonymous JSON walking.
    - Malformed roles, malformed content blocks, and missing required fields still surface `AppError::BadRequest` from the semantic boundary rather than from deep inside dispatch code.
  </behavior>
  <action>Create `src/claude/mod.rs` and `src/claude/semantic_core.rs` as the interface-first center of the Claude native gateway. Define typed semantic contracts for the slice the repo already supports: request envelope, system prompt, message list, content blocks (`text`, `tool_use`, `tool_result`), tools, tool choice, stream flag, temperature/top_p/max_tokens, and a small extensions holder for Claude-native fields already observed in real traffic (`thinking`, `context_management`, `metadata`, beta/request hints) even if those extensions are not forwarded yet. Add pure parse/normalize helpers from Anthropic JSON into the core model and expose the normalized properties the adapter needs. Keep the module transport-agnostic: no store access, no routing policy, no Axum extractors, no Reqwest request building. Export the new module from `src/lib.rs`. Do not move request logging, auth, or upstream dispatch into this task.</action>
  <verify>
    <automated>cargo test claude_semantic_core --lib</automated>
  </verify>
  <done>`src/claude/semantic_core.rs` exists, contains the typed Claude semantic model for the currently supported slice, and its parser/validation rules are locked by focused unit tests.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Build a thin Responses provider adapter around the existing mappings</name>
  <files>src/claude/mod.rs, src/claude/responses_adapter.rs, src/http.rs</files>
  <read_first>src/claude/semantic_core.rs, src/http.rs</read_first>
  <behavior>
    - A typed Claude semantic request can be converted into a Responses request payload for the currently supported slice without the caller touching raw JSON field names.
    - A non-stream Responses JSON body can be converted back into a Claude message payload with text and `tool_use` blocks.
    - Responses SSE frames can be translated into Anthropic SSE events through adapter-owned methods/state instead of free-floating helper functions in `src/http.rs`.
  </behavior>
  <action>Create `src/claude/responses_adapter.rs` and introduce an explicit `ResponsesProviderAdapter` seam that owns the provider-specific translation logic now embedded in `src/http.rs`. Move or wrap the existing pure helpers behind named adapter methods for: request mapping to `/v1/responses`, non-stream response mapping back to Claude message JSON, and stream frame adaptation back to Anthropic SSE. Make assistant-history compatibility retry an explicit adapter mode/state rather than an ad-hoc branch. Keep `Responses` as the only provider target in this slice per project decision; do not add provider selection, new upstream protocols, or database-aware behavior. If a full extraction would be too large for one quick task, leave thin delegating shims in `src/http.rs`, but make `src/claude/responses_adapter.rs` the single place new Claude<->Responses mapping code lands going forward.</action>
  <verify>
    <automated>cargo test claude_responses_adapter --lib</automated>
  </verify>
  <done>`src/claude/responses_adapter.rs` owns the Claude<->Responses mapping seam for request, non-stream response, and stream events, and focused tests pin the supported behavior.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 3: Rewire `/v1/messages` to use the new core and adapter without changing external behavior</name>
  <files>src/http.rs</files>
  <read_first>src/http.rs, src/claude/semantic_core.rs, src/claude/responses_adapter.rs</read_first>
  <behavior>
    - `create_message` parses Claude requests through the semantic core before dispatching upstream.
    - The message path still uses the existing Responses dispatch core and keeps Anthropic-style error envelopes and request-id correlation behavior from quick task `260407-oo3`.
    - Current non-stream text and stream text/tool paths keep working through the new seam, proving the extraction did not create a parallel proxy implementation.
  </behavior>
  <action>Refactor the Claude message ingress in `src/http.rs` so the file becomes an HTTP boundary plus orchestration layer: parse body, build request-log context, call the semantic core parser, hand the typed request to `ResponsesProviderAdapter`, and use adapter output when sending to the existing `proxy_request` / Responses upstream path. Preserve the current `/v1/messages` and `/messages` external behavior: Anthropic-style error envelopes, request-id correlation, Claude request fingerprint logging, and the existing supported text/tool/stream slice. Do not change route registration, auth middleware, request log schema, or `/v1/responses` and `/v1/chat/completions` behavior in this task. Add focused regression tests proving a Claude messages request still succeeds through the extracted seam for at least one non-stream path and one stream or tool path.</action>
  <verify>
    <automated>cargo test claude_native_gateway_skeleton --lib</automated>
  </verify>
  <done>The `/v1/messages -> semantic_core -> responses_adapter -> existing responses dispatch` chain is live in code, and regression tests show the refactor preserved current supported behavior while clarifying the architecture.</done>
</task>

</tasks>

<verification>
Before declaring the quick task complete:
- [ ] `cargo fmt`
- [ ] `cargo test claude_semantic_core --lib`
- [ ] `cargo test claude_responses_adapter --lib`
- [ ] `cargo test claude_native_gateway_skeleton --lib`
- [ ] `cargo test --lib`
</verification>

<success_criteria>
- Claude native gateway logic now has a named semantic core and a named Responses adapter instead of a monolithic `http.rs` implementation.
- The extraction reinforces the project decision that `/v1/responses` remains the only upstream core for Claude-native handling.
- The current Claude `/v1/messages` slice still works after the refactor, with request-id/error/fingerprint behavior preserved.
- Future Claude-native work can add providers or richer semantics behind the new seam instead of growing more ad-hoc JSON logic in `src/http.rs`.
</success_criteria>

<output>
After completion, create `/home/wcn/metapi-rs/.planning/quick/260407-ryn-claude-claude-semantic-core-responses-pr/260407-ryn-SUMMARY.md`
</output>
