---
phase: quick-260407-txr-claude-code-claude-messages-contract-tes
plan: 01
type: execute
wave: 1
depends_on:
  - quick-260407-ts1
files_modified:
  - src/claude/responses_adapter.rs
autonomous: true
requirements:
  - quick-260407
user_setup: []
must_haves:
  truths:
    - "Claude Code 对 interruption、synthetic error、tool_result 配对的关键假设被固化为回归测试。"
    - "Claude compatibility boundary 继续保持薄层，不引入第二套协议核心。"
    - "流式失败和 tool_result-only 用户消息的兼容语义可被明确验证。"
  artifacts:
    - path: src/claude/responses_adapter.rs
      provides: "source-driven tests for stream failure surfacing and tool_result pairing"
---

<objective>
基于 Claude Code 源码继续补充 Claude messages contract tests，覆盖 interruption、synthetic error 与 tool_result 配对语义。

Purpose: 继续沿着 Claude Code 源码做反向约束，把它对 `yieldMissingToolResultBlocks`、`createAssistantAPIErrorMessage` 和 `isResultSuccessful` 的关键假设转成我们网关的本地回归测试。
Output: 一次原子 quick，补充 Claude messages compatibility tests，重点覆盖 stream failure surfacing、tool_result-only 用户消息与 function_call_output 配对。
</objective>

<execution_context>
Source references consulted:
- `/home/wcn/Downloads/CC-Source/src/query.ts`
- `/home/wcn/Downloads/CC-Source/src/QueryEngine.ts`
- `/home/wcn/Downloads/CC-Source/src/utils/messages.ts`
- `/home/wcn/Downloads/CC-Source/src/utils/queryHelpers.ts`

Scope guard:
- 不修改 route/store/schema
- 不新增 upstream 协议
- 优先补 contract tests，仅在必要时做最小实现修正
</execution_context>

<verification>
- [x] `cargo test claude_responses_adapter --lib`
- [x] `cargo test claude_semantic_core --lib`
- [x] `cargo fmt`
- [x] `cargo test --lib`
</verification>
