# Design

## 核心原则

- 单二进制、轻量化、一键部署
- 服务端承接流量，TUI 负责本地或 SSH 运维
- 小而清楚地管理多个中转站，不做重型平台
- 一个 route 只对应一个下游模型名
- 一个 route 下允许多个 channel，各自可映射不同 upstream_model
- 只有一套冷却机制
- `metapi.toml` 是静态拓扑真源
- sqlite 只承载运行态 state 与日志
- 每次选路都可解释
- 每次失败都能落到明确字段

## 请求链路

```text
POST /v1/responses
  -> 解析 model
  -> 查 model_routes
  -> 加载 channels
  -> 过滤不可用渠道
  -> 在最低优先级组按 weight 选择
  -> 转发到上游 /v1/responses
  -> 写 request_logs
  -> 成功清冷却 / 失败写冷却
```

```text
POST /v1/chat/completions
  -> 校验 model/messages
  -> 转换成 responses payload
  -> 复用同一套选路与上游调用
  -> 非流式映射回 chat.completion
  -> 流式把 responses SSE 转成 chat.completion.chunk
  -> 写 request_logs
```

```text
POST /api/routes
  -> 先用真实 `/v1/responses` 请求做最小探测
  -> 如果 route_model 不存在则创建 route
  -> 归一化 base_url（兼容用户直接填写 `/v1`）
  -> 把 base_url + api_key 归入一个 channel
  -> 为该 channel 保存自己的 upstream_model
  -> 返回 route 与 channel 明细

GET /api/routes
  -> 聚合 model_routes + channels + accounts + sites
  -> 返回每条路由的渠道总数、可用数、冷却数

GET /api/routes/:id/channels
  -> 读取指定 route 的全部 channels
  -> 复用同一套 eligibility 判定逻辑
  -> 返回 state / reason / cooldown_remaining_seconds

GET /api/routes/:id/logs
  -> 按时间倒序读取该 route 的最近 request_logs
  -> join channels / accounts / sites 补齐 channel_label / site_name / upstream_model
  -> 返回统一 error_kind / error_hint 方便直接排障

DELETE /api/routes/:id
  -> 仅允许删除空 route
  -> 如果 route 下还有 channel，直接拒绝
  -> 避免把 route 删除做成危险的整组级联操作

POST /api/routes/:id/channels
  -> 校验 route 是否存在
  -> 归一化 base_url / upstream_model / priority / weight
  -> 创建站点、账号与 channel 关联
  -> 返回新 channel 明细

PATCH /api/channels/:id
  -> 归一化 base_url / api_key / upstream_model / priority / weight
  -> 复用或创建目标站点 / 账号
  -> 更新 channel 的 account_id 与路由字段
  -> 清理无引用的旧账号 / 旧站点

DELETE /api/channels/:id
  -> 删除单个 channel
  -> 若该 route 变为空，自动清理空 route
  -> 同时清理无引用的旧账号 / 旧站点

POST /api/channels/:id/enable|disable|reset-cooldown
  -> 直接更新 channel 运维状态
  -> 返回更新后的 channel 明细
```

## 流式 responses

`stream=true` 时采用“直通 + 后台收尾”：

```text
上游 reqwest stream
  -> 后台 task 逐块读取
  -> mpsc channel
  -> 下游 axum Body::from_stream
  -> 流结束后再落 request_logs / channel success
  -> 中途读流失败则落 failure + cooldown
```

## chat/completions 兼容层

这个兼容层刻意保持薄：

- 下游兼容 `messages`
- 上游固定仍走 `/v1/responses`
- 不引入第二套路由和健康状态
- 非流式只做一次 JSON 结构映射
- 流式只转换必要的 SSE 事件：
  - `response.output_text.delta` -> `chat.completion.chunk`
  - `response.output_item.added (function_call)` -> `delta.tool_calls`
  - `response.function_call_arguments.delta` -> `delta.tool_calls[].function.arguments`
  - `response.completed` -> `finish_reason=stop/tool_calls` + `[DONE]`
  - `response.failed` -> 记失败并触发冷却

兼容层当前支持的函数调用映射：

- chat `tools[].function` -> responses `tools[]`
- chat `tool_choice.function.name` -> responses `tool_choice.name`
- chat assistant `tool_calls[]` -> responses `function_call`
- chat tool role message -> responses `function_call_output`
- responses `function_call` -> chat assistant `tool_calls[]`

## 最小管理面

当前管理接口刻意不做复杂后台：

- 不做分页
- 不做认证
- 不做多维筛选

这里的管理面针对的是运行态：

- 查看当前 route/channel 的实际可用性
- 查看冷却、阻断、错误、日志
- 在运行中做临时 enable/disable/recover/edit

静态拓扑的长期归档目标仍然是 `metapi.toml`。

先优先保证一件事：

- 能直接看见每条 route 下每个 channel 为什么不可用
- 能直接看见每个 channel 最近成功/失败请求长什么样
- 能用最少操作把新的 channel 加进指定 route
- 常见错误要能直接分类成 auth / edge_blocked / upstream_path_error / rate_limited

## 冷却状态机

```text
READY
  -- 请求失败 --> COOLING_DOWN

COOLING_DOWN
  -- now >= cooldown_until --> READY

READY
  -- 请求成功 --> READY
```

## 为什么刻意做简单

原项目最容易失控的点，不是“路由功能不够多”，而是：

- 状态分散在多张表和多层内存语义里
- 同时做了概率修正、健康分、站点熔断、模型熔断、冷却
- 删除或重建通道后，行为不可预测

这个版本优先保证：

- 可以解释
- 可以测试
- 可以重放
- 可以稳定演进

## 配置与状态分层

长期目标是把系统分成三层：

```text
metapi.toml
  -> route/channel 静态配置真源

sqlite state db
  -> cooldown / fail count / manual_blocked / recent runtime edits

request logs
  -> 最近请求明细与排障上下文
```

当前代码仍然保留“启动时把配置同步进 sqlite”的过渡实现，
但设计方向已经明确：

- 配置不应长期依赖 sqlite 作为事实源
- sqlite 更适合作为轻量运行态 state store
