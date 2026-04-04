<!-- GSD:project-start source:PROJECT.md -->
## Project

**metapi-rs**

`metapi-rs` 是一个面向运维和稳定性的轻量 LLM 路由与管理工具。它采用“服务端 + TUI”产品形态：服务端部署在服务器上承接真实流量和管理状态，本地或远程通过轻量 TUI 进行排障和运维操作。产品方向强调单二进制、轻量化、一键部署、易用，核心价值是把多个中转站纳入统一路由和管理，而不是演化成重型平台。

**Core Value:** 让 LLM 路由行为足够稳定、可解释、可操作，出问题时能直接看懂并手动救火。

### Constraints

- **Tech stack**: Rust + Axum + Sqlite — 保持部署轻量，减少依赖面
- **Operability**: 状态必须能在 TUI 中直接展示和解释 — 这是工具可用性的核心
- **Compatibility**: 对下游要兼容 OpenAI 常见调用方式 — 方便替换接入
- **Simplicity**: 路由和健康策略必须少而清楚 — 避免重演原项目的复杂度失控
- **Deployment**: 默认交付形态应保持单二进制、轻量、一键部署 — 不能为了管理体验引入重型依赖
- **Dependency policy**: 依赖默认优先使用 crates.io 最新稳定版，并通过 `cargo add` 管理新增或升级；对 `0.x` 依赖升级也要跑编译和测试验证
- **Code quality**: 保持代码、设计和架构的优雅；该重构就重构，不把临时兼容堆成长期屎山
- **Non-negotiable**: 当兼容成本明显破坏当前结构时，优先删除、收敛或中止兼容，而不是继续叠补丁
<!-- GSD:project-end -->

<!-- GSD:stack-start source:STACK.md -->
## Technology Stack

Technology stack not yet documented. Will populate after codebase mapping or first phase.
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

## Naming Patterns
- Rust source files 使用 snake_case
- 二进制入口放在 `src/bin/`
- 规划与设计文档统一放在 `.planning/`
- 使用 snake_case
- handler / mapper / helper 命名直接反映职责
- 避免用模糊名承载多重行为
- 使用 snake_case
- 常量使用 UPPER_SNAKE_CASE
- 不使用含糊缩写，除非领域里已经约定俗成
- 结构体、枚举、trait 使用 PascalCase
- 请求与响应结构体名称应直接表达用途
## Code Style
- 使用 `cargo fmt`
- 保持标准 Rust 风格
- 优先小而清楚的函数和显式数据流
- 新增依赖和升级依赖优先使用 `cargo add`
- 默认优先采用 crates.io 最新稳定版，而不是手写过旧版本
- 对 `0.x` 依赖升级必须额外跑编译和测试，确认没有隐性破坏
- 至少保证 `cargo check` 和 `cargo test` 通过
- 新增行为优先补测试，而不是只靠人工解释
## Import Organization
- 组之间保留空行
- 不为了省几行把导入搅成难读的大块
## Error Handling
- 在边界层返回清晰错误，不吞错
- 错误分类要服务于运维可解释性
- 配置错误、协议错误、临时上游错误要尽量分开
## Logging
- 记录状态转换、上游调用失败、重要运维动作
- 日志和管理面展示的信息要尽量一致
## Comments
- 解释为什么这样设计，而不是代码表面在做什么
- 对兼容层、状态机、协议映射等容易误解的地方补最少必要注释
## Function Design
- 函数过长或职责混杂时，优先拆分
- 不把解析、业务规则、持久化、展示拼进一个函数
## Module Design
- 保持代码、设计和架构的优雅
- 保持部署简单，优先单二进制和轻量依赖
- 能不用重依赖就不用，避免为小功能引入整套大框架
- 该重构就重构，不把临时补丁堆成长期结构
- 不为历史行为做无限兼容
- 当兼容成本明显破坏当前结构时，优先删除、收敛或中止兼容，而不是继续叠补丁
- 兼容层必须是薄层；如果兼容逻辑开始反向污染核心模型，应优先重构边界
- 新需求若明显破坏当前结构，应先调整抽象，再继续叠功能
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd:quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd:debug` for investigation and bug fixing
- `/gsd:execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->



<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd:profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
