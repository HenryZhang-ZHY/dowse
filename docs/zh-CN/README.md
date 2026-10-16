<sub><a href="../README.md">English</a> · <strong>简体中文</strong></sub>

# dowse 文档中心

欢迎查阅 **dowse** 官方文档。dowse 是一款面向本地多代码仓库的高性能代码检索引擎，融合了 GitHub 代码搜索语法、毫秒级本地 Trigram 倒排索引以及 GPU 硬件加速的桌面图形界面。

无论你是在海量代码中探索检索的软件工程师、负责维护团队镜像仓库自动同步的架构师，还是希望为大模型编程助手（AI Coding Agent）赋予跨仓库全局代码感知能力的开发者，本套文档都将为你提供清晰详尽的指引。

---

## 核心文档导航

### 快速入门

- **[安装与更新指南](install.md)**  
  提供 Windows、macOS 与 Linux 预编译安装包的下载与配置说明。涵盖系统安全提示处理、`PATH` 环境变量配置、每日自动更新机制、配置文件目录布局以及使用 Cargo 从源码编译的完整步骤。

- **[桌面应用用户指南](app.md)**  
  基于 GPUI 构建的硬件加速桌面应用完全指南。全面介绍多工作区（Workspace）与窗口管理、仓库打标与搜索范围（Scope）限定、通过 GitHub CLI 批量克隆与自动定时拉取、代码片段与结构化表格双重视图、全文件即时语法高亮预览以及与外部编辑器的联动配置。

---

### 核心检索与索引引擎

- **[搜索语法详解](search.md)**  
  深入掌握兼容 GitHub 规范的代码查询语法：包含常规词项、精确短语、正则表达式、布尔逻辑运算（`AND`、`OR`、`NOT`）、编程语言限定符以及仓库与分支限定符。详细剖析路径漏斗过滤器与多线程分块并行搜索流水线。

- **[Trigram 索引深度解析](indexes.md)**  
  深入剖析基于 [tgrep](https://github.com/microsoft/tgrep) 的 Trigram 倒排索引架构。详解候选文件集高效剪枝机制（过滤掉 99% 以上无关文件的磁盘 I/O）、索引存储路径策略（仓库内 `.tgrep` 还是外部集中目录）、毫秒级增量更新机制以及后台实时文件监听器。

---

### 命令行工具与 AI Agent 集成

- **[命令行工具参考](cli.md)**  
  完整参考 `dowse` 所有 CLI 子命令（`search`、`repos`、`tasks`、`index`、`status`、`dev`、`settings`）。详细解析单实例 Socket IPC 架构、标准输出与错误流严格分离设计、流式 JSON（NDJSON）输出、结构化表格导出以及后台守护进程的生命周期管理。

- **[AI 编码助手集成指南](agent-guide.md)**  
  即终端运行 `dowse guide` 所输出的集成指南。专为 Claude Code、GitHub Copilot、Cursor 等大模型编程助手设计，深入讲解如何在受限的 Token 预算下利用 Facet 建议快速收窄检索范围，以及如何提取结构化代码数据。

---

### 架构设计与二次开发

- **[架构与开发指南](development.md)**  
  全面介绍 Rust 代码库内部架构（`src/engine`、`src/ui`、`src/cli`、`src/ipc`、`src/diagnostics`）。涵盖核心模块职能划分、单元测试与集成测试规范、性能基准压测工具（`examples/bench.rs`）以及基于 GitHub Actions 的多平台自动化发布流水线。

---

## 高频场景速查

| 目标场景                                         | 参考文档                                        | 推荐操作                                                              |
| ------------------------------------------------ | ----------------------------------------------- | --------------------------------------------------------------------- |
| 同时检索团队 50+ 个关联仓库                      | [桌面应用](app.md#工作区与窗口)                 | 直接将包含多个仓库的父文件夹拖入窗口，或执行 `dowse ~/src/team/*`     |
| 从海量代码中提取结构化信息（如版本号、API 路由） | [桌面应用](app.md#结果展示代码片段与表格视图)   | 使用带捕获组的正则查询，按 `Alt+T` 切入表格，一键导出为 CSV/JSON      |
| 保持本地团队 GitHub 镜像定时自动同步             | [桌面应用](app.md#从-github-克隆仓库)           | 设置定时同步周期（如 `1h`），系统将在后台静默拉取并在安全时自动快进合并 |
| 保持 Git 工作目录整洁，不产生 `.tgrep` 目录      | [索引机制](indexes.md#索引存放位置策略)         | 执行 `dowse settings set index.location external`                     |
| 将检索结果无缝接入 AI Agent 或自动化脚本         | [命令行](cli.md#面向人与-agent-的输出设计)      | 执行 `dowse search --json 'query'` 或 `dowse search -l 'query'`       |
| 仅对当前终端所在的单个项目执行针对性排查         | [搜索语法](search.md)                           | 执行 `dowse search --here 'pattern'` 或使用限定符 `repo:name pattern` |

---

## 全局高频快捷键

| 快捷键 (Win / Linux)          | 快捷键 (macOS)              | 功能说明                                   |
| ----------------------------- | --------------------------- | ------------------------------------------ |
| `Ctrl+O`                      | `Cmd+O`                     | 添加仓库或包含仓库的父文件夹               |
| `Ctrl+,`                      | `Cmd+,`                     | 打开仓库管理页面（Repositories）           |
| `Ctrl+F`                      | `Cmd+F`                     | 聚焦主搜索输入框                           |
| `Ctrl+P`                      | `Cmd+P`                     | 打开/收起路径漏斗过滤器                    |
| `Alt+T`                       | `Cmd+Alt+T`                 | 切换代码片段与结构化表格视图               |
| `Alt+C` / `Alt+W` / `Alt+R`   | `Cmd+Alt+C / W / R`         | 切换区分大小写、全字匹配、正则模式         |
| `Ctrl+B` / `Ctrl+Alt+B`       | `Cmd+B` / `Cmd+Alt+B`       | 显示/隐藏 Facets 侧边栏 / 右侧文件预览面板 |
| `Ctrl+T` / `Ctrl+W`           | `Cmd+T` / `Cmd+W`           | 新建搜索标签页 / 关闭当前标签页            |
| `Ctrl+Shift+E`                | `Cmd+Shift+E`               | 将表格视图数据导出为 CSV                   |
| `Ctrl+Shift+R`                | `Cmd+Shift+R`               | 增量更新当前范围内的全部仓库索引           |
| `Ctrl+K`（或 `Ctrl+Shift+P`） | `Cmd+K`（或 `Cmd+Shift+P`） | 唤起全局命令面板（Command Palette）        |
| `Ctrl+Shift+I` 或 `F12`       | `Cmd+Shift+I` 或 `F12`      | 打开开发者工具（日志、性能指标、最新查询） |
