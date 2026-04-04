# metapi-rs

一个从头收敛复杂度的 Rust 轻量 LLM 路由与管理工具。

`v0.2.0` 的产品边界很明确：

- 单二进制
- 轻量化
- 一键部署
- 易用
- 适合把多个中转站统一纳入路由和管理
- 保持小项目边界，不演化成重型平台

当前已经覆盖的核心能力：

- 接收 OpenAI 兼容的 `POST /v1/responses`
- 接收 OpenAI 兼容的 `POST /v1/chat/completions`
- 接收 Claude 兼容的 `POST /v1/messages`
- 每个 channel 显式绑定 `responses / chat_completions / claude` 三种协议之一
- 下游请求会按自身协议参与选路，优先走同协议直连 channel
- 支持 `stream=true` 的流式 `responses`
- 支持 `chat/completions` 到 `responses` 的兼容转换
- 支持 `chat/completions` 的 `tools / tool_calls`
- 用可解释的规则选择上游渠道
- 失败写日志，并按错误类型进入冷却或手动阻断
- 提供最小管理 API 和 SSH/TUI 运维界面
- 让一个下游模型名可以挂多个上游 channel
- 支持用 `metapi.toml` 在启动时同步 route/channel 拓扑
- 支持按错误类型配置不同冷却秒数
- 支持按错误类型要求人工处理并阻断 channel

## v0.2.0 适合什么

- 你手上有多个上游中转站，想统一成一个稳定下游模型名
- 你想要一个小服务，配一个 sqlite 文件就能跑
- 你希望在 SSH 里直接看 route、channel、冷却、最近日志

## v0.2.0 不做什么

- 不做重型 Web 后台
- 不做桌面 GUI
- 不做隐藏健康分
- 不做多层熔断叠加
- 不做复杂兼容历史包袱

## 设计边界

这个版本刻意不做下面这些复杂行为：

- 不做隐藏健康分
- 不做多层熔断叠加
- 不做 rebuild 后通道 ID 重建
- 不做多 endpoint 回退链
- 不做重型 Web 后台
- 不做桌面 GUI

## 目录

```text
src/
  app.rs         应用状态与路由注册
  bootstrap.rs   启动时同步 metapi.toml 拓扑
  config.rs      环境变量配置
  domain.rs      数据结构
  error.rs       统一错误响应
  http.rs        HTTP handlers
  routing.rs     选路状态机
  store.rs       sqlite 访问层
  lib.rs
  main.rs
migrations/
  0001_init.sql  初始表结构
  0002_manual_block.sql  手动阻断字段
examples/
  bootstrap.sql  最小种子数据
DESIGN.md
```

## 数据模型

- `sites`
- `accounts`
- `model_routes`
- `channels`
- `request_logs`

每条 `channel` 只代表一个明确的：

- 站点
- 账号
- 上游模型
- 路由优先级
- 冷却状态

这个项目的模型管理刻意保持简单：

- 一个 `route` 对应一个下游模型名，例如 `gpt-5.4`
- 一个 `route` 下面可以挂多个 `channel`
- 每个 `channel` 都可以有自己的 `upstream_model`
- 下游永远只需要请求这一个稳定的模型名

## 选路规则

1. 按 `model_routes.model_pattern` 精确匹配模型
2. 过滤掉：
   - `channel.enabled = false`
   - `account.status != active`
   - `site.status != active`
   - `manual_blocked = true`
   - `cooldown_until > now`
3. 只在最低 `priority` 的可用组里继续选路
4. 在同一个 `priority` 组内，优先选择和下游请求协议完全一致的 channel
5. 如果该 `priority` 组内没有完全一致的 channel，才会使用允许的有限转换
6. 当前只支持一种有限转换：`chat/completions -> responses`
7. 成功后清冷却
8. 失败后按错误类型写 `cooldown_until = now + cooldown_seconds`，或者直接标记 `manual_blocked = true`

## 快速开始

```bash
cd /home/wcn/metapi-rs
cp .env.example .env
cargo run
```

服务启动时会自动跑 sqlite migration。

目标部署体验是：

- 一条命令启动服务
- 一个 sqlite 文件落状态
- 一份配置接入多个上游中转站
- 本地或 SSH 下直接用 TUI 管理

## Release 构建

```bash
cd /home/wcn/metapi-rs
cargo build --release --bin metapi-rs --bin metapi-tui
```

产物位置：

- `target/release/metapi-rs`
- `target/release/metapi-tui`

如果要给 GitHub Releases 产出可直接上传的压缩包：

```bash
cd /home/wcn/metapi-rs
./scripts/build-release.sh --tag v0.2.0
```

这会生成：

- `dist/metapi-<os>-<arch>.tar.gz`
- `dist/SHA256SUMS`

推荐发布流程：

1. 用对应平台机器运行 `./scripts/build-release.sh --tag <tag>`
2. Windows 上运行 `.\scripts\build-release.ps1 -Tag <tag>`
3. 把 `dist/*.tar.gz`、`dist/*.zip` 和 `dist/SHA256SUMS` 上传到 GitHub Release
4. 在 Release 页面写清楚 server / TUI 的安装命令

Windows 打包：

```powershell
cd C:\path\to\metapi-rs
powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1 -Tag v0.2.0
```

这会生成：

- `dist/metapi-windows-x86_64.zip`
- `dist/SHA256SUMS`

## 一键安装

如果你已经把产物发到了 GitHub Releases，可以直接用安装脚本。

服务端安装：

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.sh | \
  bash -s -- --repo nodca/routellm --tag v0.2.0
```

这会：

- 下载对应平台压缩包
- 安装 `metapi-rs`
- 写入 `metapi.toml` 和 `/etc/metapi.env`
- 在 Linux 上默认安装 `systemd` 服务

常用参数：

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.sh | \
  bash -s -- \
  --repo nodca/routellm \
  --tag v0.2.0 \
  --bind 0.0.0.0:8080 \
  --master-key sk-metapi-your-key
```

TUI 安装：

```bash
curl -fsSL https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-tui.sh | \
  bash -s -- \
  --repo nodca/routellm \
  --tag v0.2.0 \
  --server http://127.0.0.1:8080 \
  --auth-key sk-metapi-your-key
```

如果你不想直接 `curl | bash`，也可以先下载脚本再执行：

```bash
curl -fsSLO https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.sh
bash install-server.sh --repo nodca/routellm --tag v0.2.0
```

Windows 服务端安装：

```powershell
powershell -ExecutionPolicy Bypass -Command "iwr https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-server.ps1 -OutFile install-server.ps1"
powershell -ExecutionPolicy Bypass -File .\install-server.ps1 -Repo nodca/routellm -Tag v0.2.0
```

Windows TUI 安装：

```powershell
powershell -ExecutionPolicy Bypass -Command "iwr https://raw.githubusercontent.com/nodca/routellm/main/scripts/install-tui.ps1 -OutFile install-tui.ps1"
powershell -ExecutionPolicy Bypass -File .\install-tui.ps1 -Repo nodca/routellm -Tag v0.2.0 -Server http://127.0.0.1:8080 -AuthKey sk-metapi-your-key
```

说明：

- Windows 安装脚本会把二进制和运行脚本安装到 `%LOCALAPPDATA%\metapi`
- 当前版本的 Windows server 先以前台运行脚本为主，不内置 Windows Service 安装
- Linux 仍然是主支持平台，Windows 当前重点是“可下载、可安装、可运行”

最小启动方式：

```bash
METAPI_BIND_ADDR=0.0.0.0:8080 \
METAPI_DATABASE_URL=sqlite:///opt/metapi/metapi-state.db \
METAPI_REQUEST_TIMEOUT_SECS=90 \
METAPI_MASTER_KEY=sk-metapi-change-me \
./target/release/metapi-rs
```

如果要用配置文件驱动启动：

```bash
METAPI_CONFIG_PATH=examples/metapi.toml ./target/release/metapi-rs
```

静态校验一个配置文件：

```bash
./target/release/metapi-rs check-config ./examples/metapi.toml
```

如果已经设置了 `METAPI_CONFIG_PATH`，也可以直接：

```bash
METAPI_CONFIG_PATH=./examples/metapi.toml ./target/release/metapi-rs check-config
```

TUI 连接已启动服务：

```bash
METAPI_BASE_URL=http://127.0.0.1:8080 \
METAPI_AUTH_KEY=sk-metapi-change-me \
./target/release/metapi-tui
```

## 环境变量

服务端：

- `METAPI_BIND_ADDR`
  - 默认 `0.0.0.0:8080`
- `METAPI_DATABASE_URL`
  - 默认 `sqlite://metapi-state.db`
  - 用来保存运行态 state 与日志
  - 不建议把它当作长期配置真源
- `METAPI_REQUEST_TIMEOUT_SECS`
  - 默认 `90`
- `METAPI_MASTER_KEY`
  - 可选
  - 配置后会保护 `/v1/*` 和 `/api/*`
  - 下游客户端和 TUI 都需要携带这个 Bearer Key
- `METAPI_CONFIG_PATH`
  - 可选
  - 指向一个 `toml` 配置文件
  - 启动时会把其中声明的 route/channel 同步到 sqlite

TUI：

- `METAPI_BASE_URL`
  - 默认 `http://127.0.0.1:8080`
- `METAPI_AUTH_KEY`
  - 可选
  - 当服务端配置了 `master_key` 时，TUI 需要配置同一个 key

`.env.example` 已给出一份最小默认值。

## 冷却与人工处理

`routing.cooldowns` 可以按错误类型覆盖默认冷却秒数。

`routing.manual_intervention` 可以把某些错误直接升级为人工处理状态，例如：

```toml
[routing.manual_intervention]
auth_error = true
upstream_path_error = true
```

被标记为人工处理的 channel 会：

- 从选路里直接跳过
- 在管理 API 和 TUI 里显示为 `manual_intervention_required`
- 通过 `POST /api/channels/{id}/reset-cooldown` 一次性清掉冷却和手动阻断

## 服务端鉴权

如果配置了 `master_key`，metapi 会对下面两类接口统一做 Bearer 鉴权：

- `/v1/*`
- `/api/*`

`/healthz` 继续保持不鉴权，方便做进程存活检查。

最简单的本机模式可以这样配：

```bash
export METAPI_MASTER_KEY=sk-metapi-local
export METAPI_AUTH_KEY=sk-metapi-local
export METAPI_BASE_URL=http://127.0.0.1:8080
```

这时：

- 下游客户端把 `Base URL` 设成 `http://127.0.0.1:8080/v1`
- `API Key` 设成你自己定义的 `sk-metapi-local`
- TUI 也用同一个 key 连本机服务

## 部署

推荐最小部署结构：

```text
/opt/metapi/
  metapi-rs
  metapi-state.db
/etc/metapi.env
```

`/etc/metapi.env` 可以直接从 [.env.example](/home/wcn/metapi-rs/.env.example) 改出来，例如：

```bash
METAPI_BIND_ADDR=0.0.0.0:8080
METAPI_DATABASE_URL=sqlite:///opt/metapi/metapi-state.db
METAPI_REQUEST_TIMEOUT_SECS=90
```

`systemd` 示例见 [examples/metapi.service](/home/wcn/metapi-rs/examples/metapi.service)。
配置文件示例见 [examples/metapi.toml](/home/wcn/metapi-rs/examples/metapi.toml)。

最小安装流程：

```bash
sudo mkdir -p /opt/metapi
sudo cp target/release/metapi-rs /opt/metapi/
sudo cp examples/metapi.service /etc/systemd/system/metapi.service
sudo cp .env.example /etc/metapi.env
sudo systemctl daemon-reload
sudo systemctl enable --now metapi
```

如果要看运行状态：

```bash
systemctl status metapi
journalctl -u metapi -f
```

## TUI

先启动 API 服务，再开 TUI：

```bash
cd /home/wcn/metapi-rs
METAPI_BASE_URL=http://127.0.0.1:8080 cargo run --bin metapi-tui
```

当前 TUI 功能：

- 左侧查看 routes
- 右侧上半区查看当前 route 的 channels
- 右侧下半区查看当前 route 最近请求日志
- `Tab` 在 routes / channels / logs 三个面板间循环切换
- 左右方向键在 pane 间移动焦点
- 上下方向键移动当前列表选中项
- `Home / End` 跳到当前列表顶部 / 底部
- `PgUp / PgDn` 快速滚动长列表
- `/` 只过滤左侧 routes
- `u` 复制下游 `Base URL`
- `K` 复制当前配置的下游 `API Key`
- `r` 刷新
- `a` 在 routes 面板新建 route + 首个 channel，在 channels / logs 面板为当前 route 新增 channel
- `i` 编辑当前 channel 的 `base_url / api_key / upstream_model / protocol / priority`
- `x` 在 routes 面板删除空 route，在 channels 面板删除当前 channel，都会先确认
- `Space` 一键切换当前 channel 状态
- `e` 启用当前 channel，会先确认
- `d` 禁用当前 channel，会先确认
- `c` 恢复当前 channel，会清掉冷却和手动阻断，并先确认
- `Enter` 在 routes 面板进入 channels，在 channels / logs 面板打开详情弹窗
- `Enter / y` 确认当前弹窗动作
- `Esc / n` 取消当前弹窗动作
- `?` 查看帮助
- `q` 退出

说明：

- TUI 里只会展示 key 的掩码形式，例如前 4 位和后 4 位
- Linux / macOS 下复制默认走终端剪贴板协议（OSC52）
- Windows 下复制走系统剪贴板
- 本机终端通常可直接用，SSH 场景下取决于你的终端是否允许远程复制

当前 TUI 的目标不是做配置后台，而是让你在 SSH 里快速回答这几个问题：

- 现在哪个 route 还能跑
- 哪个 channel 正在冷却或被阻断
- 刚才的请求走到了哪里
- 我现在应该恢复、禁用，还是直接删掉这个 channel

## 初始化示例数据

```bash
sqlite3 metapi-state.db < examples/bootstrap.sql
```

## 配置文件启动

`v0.2` 当前最重要的新能力，是用一个 `toml` 文件声明路由拓扑。

最小示例：

```toml
[server]
bind_addr = "0.0.0.0:8080"
database_url = "sqlite://metapi.db"
database_url = "sqlite://metapi-state.db"
request_timeout_secs = 90

[routing]
default_cooldown_seconds = 300

[[routes]]
model = "gpt-5.4"

[[routes.channels]]
base_url = "https://api.example.com/v1"
api_key = "sk-xxx"
upstream_model = "gpt-5.4"
protocol = "responses"
priority = 0
enabled = true
```

启动方式：

```bash
METAPI_CONFIG_PATH=./examples/metapi.toml cargo run
```

当前同步规则：

- `route.model` 作为 route 唯一键
- `channel` 用 `route + account(base_url+api_key) + upstream_model` 识别
- 配置里有、数据库里没有：创建
- `channel.protocol` 必填，只能是 `responses`、`chat_completions`、`claude`
- 配置里有、数据库里已有：更新 `cooldown_seconds`、`protocol`、`priority`、`enabled`
- 配置里没有、数据库里已有：当前版本不删除
- 运行时冷却状态、失败计数、最近错误、请求日志都会保留

这一步可以把它理解成：

- `metapi.toml` 管静态拓扑
- sqlite 管运行态 state
- 当前版本仍然通过“启动时同步”把两者接起来

也就是说，sqlite 现在更接近 `state.db`，而不是配置真源。

分层冷却配置示例：

```toml
[routing.cooldowns]
auth_error = 1800
rate_limited = 45
upstream_server_error = 300
transport_error = 30
edge_blocked = 1800
upstream_path_error = 1800
unknown_error = 300
```

说明：

- 如果某个错误类型配置了专属秒数，就优先用这个值
- 如果没配置，就回退到该 route 自己的 `cooldown_seconds`

## 接口

- `GET /healthz`
- `GET /api/routes/decision?model=gpt-5.4`
- `GET /api/routes`
- `POST /api/routes`
- `DELETE /api/routes/:id`
- `GET /api/routes/:id/channels`
- `GET /api/routes/:id/logs`
- `POST /api/routes/:id/channels`
- `GET /api/channels/:id/prefill`
- `PATCH /api/channels/:id`
- `DELETE /api/channels/:id`
- `POST /api/channels/:id/enable`
- `POST /api/channels/:id/disable`
- `POST /api/channels/:id/reset-cooldown`
- `POST /v1/responses`
- `POST /v1/chat/completions`
- `POST /v1/messages`

## 最小管理 API

当前管理面先以排障和最小写操作为主，不做重后台：

- `GET /api/routes`
  - 列出所有模型路由
  - 返回每条路由的 `channel_count`
  - 返回 `ready_channel_count` 和 `cooling_channel_count`
- `POST /api/routes`
  - 一步完成 route + channel onboarding
  - 如果 `route_model` 不存在就创建 route
  - 如果已存在就直接把新的 channel 加到该 route
  - 每个 channel 可以设置自己的 `upstream_model`
  - `protocol` 必填，只能是 `responses` / `chat_completions` / `claude`
  - `base_url` 可以填写站点根地址，也可以直接填写带 `/v1` 的兼容地址
  - 保存前会按该 channel 的 `protocol` 做一次真实探测，失败不会保存
- `GET /api/routes/:id/channels`
  - 列出该路由下全部渠道
  - 直接返回 `state`
  - 直接返回 `reason`
  - 直接返回 `cooldown_until` / `cooldown_remaining_seconds`
  - 直接返回 `last_status` / `last_error`
  - 直接返回 `last_error_kind` / `last_error_hint`
- `GET /api/routes/:id/logs`
  - 列出该 route 最近请求日志，默认 20 条
  - 可用 `?limit=` 控制返回条数，范围 1 到 100
  - 直接返回 `channel_label` / `site_name` / `upstream_model`
  - 直接返回 `http_status` / `latency_ms` / `error_message`
  - 直接返回 `error_kind` / `error_hint`
- `DELETE /api/routes/:id`
  - 只允许删除空 route
  - 如果 route 下面还有 channel，会直接返回错误，避免误删整组渠道
- `POST /api/routes/:id/channels`
  - 向指定 route 新增 channel
  - 接收 `base_url` 和 `api_key`
  - 可选传入 `upstream_model` / `priority`
  - `protocol` 必填，只能是 `responses` / `chat_completions` / `claude`
  - `base_url` 若带 `/v1` 会自动归一化
- `GET /api/channels/:id/prefill`
  - 返回单个 channel 的 `base_url` / `api_key` / `upstream_model`
  - 只给 TUI 的“沿用当前 channel 新增 sibling channel”预填使用
- `PATCH /api/channels/:id`
  - 编辑单个 channel 的 `base_url` / `api_key` / `upstream_model` / `protocol` / `priority`
  - 如果改了 `base_url` 或 `api_key`，会把 channel 重新绑定到对应账号，并清理无引用的旧账号 / 旧站点
- `DELETE /api/channels/:id`
  - 直接删除单个 channel
  - 如果删掉的是该 route 最后一个 channel，会自动清理掉空 route
- `POST /api/channels/:id/enable`
  - 直接启用指定 channel
- `POST /api/channels/:id/disable`
  - 直接禁用指定 channel
- `POST /api/channels/:id/reset-cooldown`
  - 直接清除冷却并重置失败计数

## 流式行为

当请求里带 `stream=true` 时：

- 服务不会缓冲完整响应
- 会把上游 SSE 数据块直接转发给下游
- 会在流真正结束后再写成功日志
- 如果上游流中途断掉，会记失败日志并触发冷却

`/v1/chat/completions` 会复用同一条上游链路：

- 非流式请求先转换成 `responses.input`
- 上游 JSON 响应再映射回 `chat.completion`
- 流式请求会把上游 `responses` SSE 事件转换成 `chat.completion.chunk`
- 实际上游仍然只调用 `/v1/responses`
- `tools` 会从 chat 的 function tool 结构映射到 responses tool 结构
- assistant `tool_calls` 会映射成 responses `function_call`
- tool 消息会映射成 responses `function_call_output`
- 上游 `function_call` 会映射回 chat `tool_calls`

## 开发验证

```bash
cargo fmt
cargo test
```
