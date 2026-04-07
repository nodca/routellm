# Quick Task 260407-txr Summary

## Task

基于 Claude Code 源码继续补充 Claude messages contract tests，覆盖 interruption、synthetic error 与 tool_result 配对语义。

## Source Findings

从 [query.ts](/home/wcn/Downloads/CC-Source/src/query.ts)、[QueryEngine.ts](/home/wcn/Downloads/CC-Source/src/QueryEngine.ts)、[messages.ts](/home/wcn/Downloads/CC-Source/src/utils/messages.ts) 和 [queryHelpers.ts](/home/wcn/Downloads/CC-Source/src/utils/queryHelpers.ts) 提炼出几个关键约束：

- 如果已经出现 `tool_use`，无论是 runtime throw 还是 streaming abort，Claude Code 都会努力补齐对应的 `tool_result`，避免悬空调用。
- 模型/运行时错误会走 synthetic assistant API error 路径，而不是伪装成 user interruption。
- `isResultSuccessful()` 把 “最后一条消息是只包含 `tool_result` blocks 的 user message” 视为成功形态之一。
- 因此对我们这个网关来说，最关键的 server-side 契约是：
  - tool_result-only 用户消息必须稳定转成 `function_call_output`
  - 上游 stream failure 必须明确冒出来，不能伪装成正常收尾

## Result

- 在 [responses_adapter.rs](/home/wcn/metapi-rs/src/claude/responses_adapter.rs) 新增了 2 个源码驱动测试：
  - `claude_responses_adapter_maps_tool_result_only_user_message_to_function_call_outputs`
  - `claude_responses_adapter_surfaces_stream_failure_for_client_side_error_synthesis`
- 这两个测试分别锁住了：
  - tool_result-only 用户消息会生成纯 `function_call_output` 序列，并保留 `call_id`
  - `response.failed` / top-level `error` 帧会明确变成 stream error，交给客户端走 synthetic error / interruption 恢复逻辑
- 这轮没有新增实现分支，现有适配器行为已经满足这些契约。

## Verification

- `cargo test claude_responses_adapter --lib`
- `cargo test claude_semantic_core --lib`
- `cargo fmt`
- `cargo test --lib`

## Notes

- 到这一步，Claude messages 兼容层已经连续补上了：
  - strict / compat profile + fallback 决策
  - stream 终止语义与 event 顺序
  - interruption / synthetic error / tool_result pairing 契约
- 后续如果继续逼近原生体验，下一块更值得做的是把真实 Claude Code 请求样本沉淀成 fixture，做更端到端的 golden tests。
