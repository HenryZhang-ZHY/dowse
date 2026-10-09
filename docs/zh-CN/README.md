<p align="right">
  <a href="../README.md">English</a> | <strong>简体中文</strong>
</p>

# dowse 文档中心

欢迎查阅 **dowse** 官方文档。dowse 是一款面向本地多代码仓库的高性能代码检索引擎，融合了 GitHub 代码搜索语法、毫秒级本地 Trigram（三元语法）倒排索引以及 GPU 加速的桌面图形界面。

无论你是需要在百万行代码间快速穿梭的研发工程师、需要维护团队镜像仓库自动同步的架构师，还是希望为大模型编程助手（AI Coding Agent）赋予全仓库全局感知能力的开发者，本套文档都将为你提供详尽的指引。

---

## 核心文档导航

### 快速入门

- **[安装与更新指南](install.md)**  
  提供 Windows、macOS 与 Linux 预编译二进制安装包的下载和配置说明。涵盖初次启动的安全提示处理、终端环境变量 `PATH` 配置、每日静默自动更新机制、配置存储路径及通过 Cargo 从源码构建的完整步骤。

- **[桌面应用用户指南](app.md)**  
  基于 GPUI 构建的现代化桌面界面完全指引。全面介绍多工作区（Workspace）与窗口管理、仓库打标与检索范围（Scope）、从 GitHub 批量克隆与自动定时同步、代码片段与结构化表格双重视图、全文件即时预览与外部编辑器无缝联动。

---

### 核心检索与索引引擎

- **[搜索语法详解](search.md)**  
  掌握兼容 GitHub 规范的高级查询语法：包含常规词项、引号精确短语、正则表达式、布尔逻辑（`AND`、`OR`、`NOT`）、语言限定符（`lang:rust`）及仓库/分支限定符。深入剖析路径漏斗过滤器与内部多线程分块搜索流水线。

- **[Trigram 索引深度解析](indexes.md)**  
  深入解析基于 [tgrep](https://github.com/microsoft/tgrep) 的 Trigram 索引架构。详解候选文件集快速剪枝技术（免去 99%+ 不必要的磁盘 I/O）、索引存储位置策略（仓库内 `.tgrep` vs 外部集中存储目录）、数秒内完成的增量更新机制以及后台实时文件监听器。

---

### 命令行工具与 AI Agent 集成

- **[命令行工具参考](cli.md)**  
  完整覆盖 `dowse` 全部子命令（`search`、`repos`、`tasks`、`index`、`status`、`dev`、`settings`）。详细解析单实例 Socket IPC 通信机制、标准输出/错误流分离设计、流式 JSON 行输出、表格格式导出与后台守护进程生命周期。

- **[AI 编码助手集成指南](agent-guide.md)**  
  即终端执行 `dowse guide` 时输出的实战指南。专为 Claude Code、GitHub Copilot、Cursor 等大模型编码助手设计，深入讲解如何在严格的 Token 预算下（`-n 100`、`-m 20`）利用 Facet 动态缩减搜索范围并提取结构化代码数据。

---

### 架构设计与二次开发

- **[架构与开发指南](development.md)**  
  全景展现 Rust 源码仓库结构（`src/engine`、`src/ui`、`src/cli`、`src/ipc`、`src/diagnostics`）。包含核心模块职能划分、单元与集成测试策略、基准性能压测工具（`examples/bench.rs`）及基于 GitHub Actions 的多平台自动发布流水线。

---

## 高频场景速查

| 目标场景                                         | 参考文档                                        | 推荐操作                                                              |
| ------------------------------------------------ | ----------------------------------------------- | --------------------------------------------------------------------- |
| 同时检索团队 50+ 个关联仓库                      | [桌面应用](app.md#工作区与窗口)                 | 直接将包含多个仓库的父文件夹拖入窗口，或执行 `dowse ~/src/team/*`     |
| 从海量代码中提取结构化信息（如版本号、API 路由） | [桌面应用](app.md#结果展示)                     | 使用带捕获组的正则查询，按 `Alt+T` 切入表格，一键导出为 CSV/JSON      |
| 保持本地团队 GitHub 镜像定时自动同步             | [桌面应用](app.md#从-github-克隆仓库)           | 对克隆的仓库设置同步频率（如 `1h`），系统将在后台无干扰安全拉取       |
| 让 Git 工作副本保持干净，不产生 `.tgrep` 目录    | [索引机制](indexes.md#将索引集中存放在仓库外部) | 执行 `dowse settings set index.location external`                     |
| 将检索结果直接灌入 AI Agent 或自动化脚本         | [命令行](cli.md#面向人与-agent-的输出设计)      | 执行 `dowse search --json 'query'` 或 `dowse search -l 'query'`       |
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
| `Ctrl+B` / `Ctrl+Alt+B`       | `Cmd+B` / `Cmd+Alt+B`       | 显示/隐藏 Facets 侧边栏 / 预览窗格         |
| `Ctrl+T` / `Ctrl+W`           | `Cmd+T` / `Cmd+W`           | 新建搜索标签页 / 关闭当前标签页            |
| `Ctrl+Shift+E`                | `Cmd+Shift+E`               | 将表格视图数据导出为 CSV                   |
| `Ctrl+Shift+R`                | `Cmd+Shift+R`               | 增量更新当前范围内的全部仓库索引           |
| `Ctrl+K`（或 `Ctrl+Shift+P`） | `Cmd+K`（或 `Cmd+Shift+P`） | 唤起全局命令面板（Command Palette）        |
| `Ctrl+Shift+I` 或 `F12`       | `Cmd+Shift+I` 或 `F12`      | 打开开发者工具（日志、性能指标、最新查询） |
