# llmrouter

一个轻量、优雅、具有确定性选路逻辑的 LLM 路由网关。

`llmrouter` 面向个人开发者和小团队，专注解决三个实际问题：

- 多个上游中转站不稳定，挂了就得手工切换
- 不同上游的模型名不统一，下游接入很痛苦
- 只想要一个小而稳的工具，不想上重型平台

它不是一个大而全的 AI 平台，而是一个小而清楚的中间层：

- 服务端承接下游请求
- TUI 负责本地或远程运维
- 单二进制部署
- 显式协议绑定
- `priority` 决定选路
- 规则可解释，状态可观察

## 为什么是 llmrouter

在拥有多个 LLM 上游时，你通常会遇到这些问题：

- 某个通道挂了，下游客户端得改 `Base URL` 或 `API Key`
- 同一个模型在不同供应商那里名字不同，例如 `gpt-5.4` / `gpt-5-4`
- 管理和排障常常依赖脚本、数据库、面板，心智负担很重

`llmrouter` 的设计重点不是“功能越多越好”，而是：

- 确定性：抛弃复杂权重、随机策略和黑盒修正
- 显式协议：每个 channel 必须明确指定上游协议，不靠猜
- 极简部署：Rust 单二进制，支持 release 下载和安装脚本
- SSH 友好：TUI 直接回答“谁在跑、谁挂了、刚才请求去哪了”

## 核心概念

| 概念 | 说明 |
| --- | --- |
| Route | 稳定的下游模型名，例如 `gpt-5.4`。客户端始终请求它。 |
| Channel | Route 下的一条具体上游通道，包含 `base_url`、`api_key`、`upstream_model`、`protocol`、`priority`。 |
| Protocol | 显式指定的上游协议类型，只能是 `responses`、`chat_completions`、`claude`。 |
| Priority | 选路优先级，数值越小越优先。只在最小 `priority` 组内继续选路。 |
| Cooldown | 自动冷却机制。运行时请求失败后进入倒计时，时间到后自动恢复。 |

## 当前能力

- 支持下游入口：
  - `POST /v1/responses`
  - `POST /v1/chat/completions`
  - `POST /v1/messages`
- 支持上游协议：
  - `responses`
  - `chat_completions`
  - `claude`
- 支持流式与非流式转发
- 支持 `chat/completions` 的 `tools / tool_calls`
- 支持 Claude `messages`
- 支持最小管理 API
- 支持 SSH/TUI 运维
- 支持 `t / T` 主动测活
- 支持 `llmrouter.toml` 启动导入
- 支持 `master_key` 保护 `/v1/*` 与 `/api/*`
- 支持 Linux / Windows 发布资产

## 明确不做什么

为了保持轻量和可维护，当前版本明确不做：

- Web UI
- 多用户系统
- 计费、配额、多租户
- 复杂统计大屏
- 插件系统
- 多层熔断、健康分、概率修正
- 为兼容历史行为而长期保留多套路径

## 路由与协议规则

### 显式协议

每个 channel 都必须显式绑定一个上游协议：

- `responses`
- `chat_completions`
- `claude`

添加 channel 时，`protocol` 是必填项，不再默认 `responses`。

### 当前兼容矩阵

| 下游请求 | 上游 channel |
| --- | --- |
| `responses` | `responses` |
| `chat_completions` | `chat_completions` |
| `claude` | `claude` |
| `chat_completions` | `responses`，通过薄适配层兼容 |

当前不支持：

- `responses -> chat_completions`
- `responses -> claude`
- `claude -> responses`
- `claude -> chat_completions`

### 选路规则

`llmrouter` 采用“确定性优先”的选路风格：

1. 根据请求里的 `model` 精确匹配 route
2. 过滤不可用 channel：
   - `channel.enabled = false`
   - account inactive
   - site inactive
   - manual blocked
   - cooldown 未到期
   - protocol 不兼容
3. 取最小 `priority` 的可用组
4. 在同优先级组内，优先选择协议直连的 channel
5. 只有同优先级组内没有直连时，才尝试有限协议适配
6. 若仍有多个候选，则按添加顺序落到具体 channel

已移除 `weight`。当前选路是明确、稳定、可解释的，不是随机加权。

## 状态模型

TUI 与管理面主要展示四种状态：

| 状态 | 说明 |
| --- | --- |
| `RUN` | 通道可用 |
| `COOL 23s` | 运行时请求失败后进入自动冷却，显示剩余秒数 |
| `UNAVAIL` | 不可用，通常是手动阻断或主动测活失败 |
| `OFF` | 手动禁用，不参与选路 |

### 主动测活语义

- `t`：测活当前 channel
- `T`：顺序测活当前 route 下全部 channel
- 测活成功：恢复为 `RUN`
- 测活失败：默认进入 `UNAVAIL`
- `COOL` 主要保留给真实请求路径里的自动冷却，不作为主动测活失败的默认结果

### 错误分类

当前主要错误类型：

- `auth_error`
- `rate_limited`
- `upstream_server_error`
- `transport_error`
- `edge_blocked`
- `upstream_path_error`
- `unknown_error`

可以按错误类型配置不同冷却秒数，也可以让某些错误直接进入人工处理状态。

## 快速开始

### 方式一：直接下载 Release

发布页：

- <https://github.com/nodca/routellm/releases>

当前提供：

- Linux `x86_64`
- Windows `x86_64`

### 方式二：安装脚本

#### Linux 单机模式

本机 server + 本机 TUI，一次装好。server 默认只监听本机 `127.0.0.1:1290`，并启用 systemd 自启动。

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-local.sh | \
  sudo bash -s -- --repo nodca/routellm --tag v1.0
```

脚本会自动：

- 安装 `llmrouter` server
- 配置 systemd 开机自启
- 为当前登录用户安装 `llmrouter-tui`
- 把本机地址和同一把管理 `KEY` 写进 TUI 配置

#### Linux Server

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.sh | \
  sudo bash -s -- --repo nodca/routellm --tag v1.0
```

默认安装到 `/opt/llmrouter`，适合 root 管理的服务端部署。非 root 安装建议改成：

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.sh | \
  bash -s -- --repo nodca/routellm --tag v1.0 \
    --install-dir "$HOME/.local/share/llmrouter" \
    --env-file "$HOME/.config/llmrouter/server.env" \
    --skip-systemd
```

#### Linux TUI

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-tui.sh | \
  bash -s -- --repo nodca/routellm --tag v1.0 --server http://127.0.0.1:1290
```

#### Windows Server

```powershell
powershell -ExecutionPolicy Bypass -Command "iwr https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.ps1 -OutFile install-server.ps1"
powershell -ExecutionPolicy Bypass -File .\install-server.ps1 -Repo nodca/routellm -Tag v1.0
```

Windows 默认安装到 `%LOCALAPPDATA%\llmrouter`。

#### Windows TUI

```powershell
powershell -ExecutionPolicy Bypass -Command "iwr https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-tui.ps1 -OutFile install-tui.ps1"
powershell -ExecutionPolicy Bypass -File .\install-tui.ps1 -Repo nodca/routellm -Tag v1.0 -Server http://127.0.0.1:1290 -AuthKey sk-llmrouter-your-key
```

Windows TUI 默认也放在 `%LOCALAPPDATA%\llmrouter`。

## 启动方式

### 直接启动服务端

```bash
LLMROUTER_BIND_ADDR=127.0.0.1:1290 \
LLMROUTER_DATABASE_URL=sqlite://./llmrouter-state.db \
LLMROUTER_MASTER_KEY=sk-llmrouter-local \
LLMROUTER_CONFIG_PATH=./examples/llmrouter.toml \
./target/release/llmrouter
```

### 校验配置文件

```bash
./target/release/llmrouter check-config ./examples/llmrouter.toml
```

如果已经设置了 `LLMROUTER_CONFIG_PATH`，也可以：

```bash
LLMROUTER_CONFIG_PATH=./examples/llmrouter.toml ./target/release/llmrouter check-config
```

### 启动 TUI

```bash
LLMROUTER_BASE_URL=http://127.0.0.1:1290 \
LLMROUTER_AUTH_KEY=sk-llmrouter-local \
./target/release/llmrouter-tui
```

## 部署方式

### 1. 本机一体化

适合个人本地使用：

- server 和 TUI 都在同一台机器
- TUI 连接本机 `http://127.0.0.1:1290`

### 2. 远程 Server + 本地 TUI

适合常见的“服务器承接流量，本地管理”：

- server 部署在 Linux VPS 或常开机器
- TUI 在本地电脑运行
- TUI 通过 `LLMROUTER_BASE_URL` 连接远程 server
- TUI 通过 `LLMROUTER_AUTH_KEY` 使用同一个 `master_key`

示例：

```bash
LLMROUTER_BASE_URL=http://your-server-ip:8080 \
LLMROUTER_AUTH_KEY=sk-llmrouter-your-key \
llmrouter-tui
```

## TUI 是什么，不是什么

TUI 是 `llmrouter` 的应用级管理终端。

它可以管理：

- route
- channel
- 启用 / 禁用 / 恢复
- 主动测活
- 查看日志
- 新增 / 编辑 / 删除 channel

它不能直接管理：

- systemd 服务启停
- server 进程重启
- 系统端口、系统环境、主机资源

也就是说：

- TUI 可以远程管理 server 里的“应用状态”
- 但不能替代 `systemctl`、SSH 或系统级运维工具

## TUI 界面与快捷键

### 界面布局

- 左侧：Routes
- 右上：Channels
- 右下：Logs
- 底部：Status

### 常用快捷键

| 按键 | 功能 |
| --- | --- |
| `Tab` | 在 Routes / Channels / Logs 之间切换 |
| `Left/Right` | 左右切换 pane |
| `Up/Down` | 移动选中项 |
| `Home/End` | 跳到顶部 / 底部 |
| `PgUp/PgDn` | 快速翻页 |
| `/` | 过滤 routes |
| `a` | 新增 route 或 channel |
| `i` | 编辑当前 channel |
| `x` | 删除当前项 |
| `Space` | 快速切换 channel 状态 |
| `e` | 启用 channel |
| `d` | 禁用 channel |
| `c` | 恢复 channel，清掉冷却 / 阻断 |
| `t` | 主动测活当前 channel |
| `T` | 主动测活当前 route 下全部 channel |
| `u` | 复制下游 Base URL |
| `K` | 复制下游 API Key |
| `Enter` | 进入或查看详情 |
| `Esc` | 返回 / 取消 |
| `y / n` | 确认弹窗 |
| `r` | 刷新 |
| `?` | 帮助 |
| `q` | 退出 |

## 配置文件

`llmrouter` 支持用 `llmrouter.toml` 描述静态拓扑，sqlite 负责保存运行态状态和日志。

最小示例：

```toml
[server]
bind_addr = "0.0.0.0:8080"
database_url = "sqlite://llmrouter-state.db"
request_timeout_secs = 90
master_key = "sk-llmrouter-change-me"

[routing]
default_cooldown_seconds = 300

[routing.cooldowns]
auth_error = 1800
rate_limited = 45
upstream_server_error = 300
transport_error = 30
edge_blocked = 1800
upstream_path_error = 1800
unknown_error = 300

[routing.manual_intervention]
auth_error = true
upstream_path_error = true

[[routes]]
model = "gpt-5.4"
cooldown_seconds = 300

[[routes.channels]]
base_url = "https://api.example.com/v1"
api_key = "sk-example-primary"
upstream_model = "gpt-5.4"
protocol = "responses"
priority = 0
enabled = true

[[routes.channels]]
base_url = "https://api-backup.example.com/v1"
api_key = "sk-example-backup"
upstream_model = "gpt-5-4"
protocol = "chat_completions"
priority = 1
enabled = true
```

### channel 字段说明

| 字段 | 说明 |
| --- | --- |
| `base_url` | 上游站点根地址，可直接填写带 `/v1` 的兼容地址 |
| `api_key` | 上游 key |
| `upstream_model` | 实际发给上游的模型名 |
| `protocol` | 必填，只能是 `responses` / `chat_completions` / `claude` |
| `priority` | 必须 `>= 0`，越小越优先 |
| `enabled` | 是否启用 |

## 环境变量

### 服务端

- `LLMROUTER_BIND_ADDR`
- `LLMROUTER_DATABASE_URL`
- `LLMROUTER_REQUEST_TIMEOUT_SECS`
- `LLMROUTER_MASTER_KEY`
- `LLMROUTER_CONFIG_PATH`

### TUI

- `LLMROUTER_BASE_URL`
- `LLMROUTER_AUTH_KEY`

## 管理 API

### 健康检查

- `GET /healthz`

### 管理接口

- `GET /api/routes/decision?model=...`
- `GET /api/routes`
- `POST /api/routes`
- `DELETE /api/routes/:id`
- `GET /api/routes/:id/channels`
- `GET /api/routes/:id/logs`
- `POST /api/routes/:id/channels`
- `GET /api/channels/:id/prefill`
- `POST /api/channels/:id/probe`
- `PATCH /api/channels/:id`
- `DELETE /api/channels/:id`
- `POST /api/channels/:id/enable`
- `POST /api/channels/:id/disable`
- `POST /api/channels/:id/reset-cooldown`

## 下游鉴权

如果设置了 `master_key`，以下接口会统一要求 Bearer Token：

- `/v1/*`
- `/api/*`

`/healthz` 不鉴权。

最简单的本机模式可以这样配：

```bash
export LLMROUTER_MASTER_KEY=sk-llmrouter-local
export LLMROUTER_AUTH_KEY=sk-llmrouter-local
export LLMROUTER_BASE_URL=http://127.0.0.1:1290
```

此时：

- 下游客户端把 `Base URL` 设为 `http://127.0.0.1:1290/v1`
- `API Key` 设为 `sk-llmrouter-local`
- TUI 也使用同一个 key 连接服务端

## 协议支持

### 下游入口

- OpenAI `responses`
- OpenAI `chat/completions`
- Claude `messages`

### 上游能力

- `responses`
  - 支持流式与非流式
- `chat_completions`
  - 支持原生直连
  - 也支持通过薄兼容层适配到 `responses`
- `claude`
  - 上游走 `/v1/messages`
  - 使用 `x-api-key`
  - 自动带 `anthropic-version: 2023-06-01`

### `chat/completions` 薄兼容层

当前已经支持：

- chat 请求映射到 responses
- 非流式 JSON 映射回 chat completion
- 流式 SSE 映射为 `chat.completion.chunk`
- `tools / tool_calls`

## 配置导入与运行态

设计上：

- `llmrouter.toml` 是静态拓扑真源
- sqlite 负责运行态状态与日志

当前实现是：

- server 启动时读取 `LLMROUTER_CONFIG_PATH`
- 把 route/channel 同步进 sqlite
- 运行过程中继续在 sqlite 里维护：
  - cooldown
  - fail count
  - manual blocked
  - request logs

也就是说，当前版本还不是纯文件模式，sqlite 仍然是必要组件。

## 当前限制

- 还没有独立的测活面板
- `T` 是 TUI 顺序调用单通道 probe，不是专门的异步批量任务
- 没有 Web UI
- 没有多用户 / token 管理系统
- 没有复杂批量管理功能
- 没有 alias 模型系统
- 没有自动协议探测
- 还没有完全去 sqlite 化
- Windows 已可发布和运行，但主支持平台仍然更偏 Linux

## 开发验证

```bash
cargo fmt
cargo test
```

当前测试覆盖：

- 配置解析
- 协议校验
- 路由选择
- responses / chat / claude 转发
- tools / tool_calls 映射
- 冷却与人工阻断
- 主动 probe 成功 / 失败
- 管理 API
- `master_key`
- TUI 部分辅助逻辑

## License

MIT
