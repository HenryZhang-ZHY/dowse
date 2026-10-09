<p align="right">
  <a href="../development.md">English</a> | <strong>简体中文</strong>
</p>

# 架构设计、贡献开发与发布流程

本文档详尽介绍了 dowse 代码库的内部架构设计、模块划分原则、测试规范以及持续集成与发布工程流。

- [开发起步与本地构建](#开发起步与本地构建)
- [核心架构设计原则](#核心架构设计原则)
- [代码库目录结构与模块说明](#代码库目录结构与模块说明)
  - [检索引擎核心库 (src/engine)](#检索引擎核心库-srcengine)
  - [桌面图形界面 (src/ui)](#桌面图形界面-srcui)
  - [命令行交互端 (src/cli)](#命令行交互端-srccli)
  - [进程间通信 (src/ipc)](#进程间通信-srcipc)
  - [系统诊断与指标 (src/diagnostics)](#系统诊断与指标-srcdiagnostics)
  - [系统平台集成与辅助模块](#系统平台集成与辅助模块)
- [自动化测试策略](#自动化测试策略)
- [性能基准压测](#性能基准压测)
- [自动化发布工作流](#自动化发布工作流)

---

## 开发起步与本地构建

编译构建 dowse 需要较新的稳定版 Rust 工具链（支持 Rust 2024 Edition，在 `mise.toml` 中配置为 `latest`）。关于操作系统特定底层依赖，请参阅 [安装、更新与环境配置](install.md#从源码编译构建)。

```bash
# 以 Release 模式编译
cargo build --release

# 针对目标代码仓库直接运行本地开发版本
cargo run --release -- path/to/repo path/to/folder-of-repos
```

---

## 核心架构设计原则

dowse 采用解耦的无头（Headless）核心引擎加硬件加速表现层的设计模式：

- **引擎层零 UI 依赖**：`src/engine/` 完全不依赖 GPUI 或任何窗口渲染组件，可作为独立的 Rust 库直接编译、单测或被其他工具链集成。
- **单实例 Socket IPC 通信**：无论用户从桌面图标启动 GUI，还是在多个不同的终端窗口频繁运行 CLI 子命令，均通过系统本地 Socket 复用同一个主进程，实现内存热索引共享。
- **Rayon 线程池分块并发**：对候选文件的磁盘读取与逐行正则匹配完全基于 Rayon 工作窃取线程池实现，最大化榨干多核 CPU 算力。
- **GPU 硬件加速渲染**：图形界面基于 Zed 开源的 GPUI 渲染框架，文本与 UI 布局直接交由显卡完成管线渲染，保证亚毫秒级帧耗时与丝滑响应。

---

## 代码库目录结构与模块说明

### 检索引擎核心库 (`src/engine/`)

负责处理语法编译、Trigram 索引交互、Git 仓库同步及状态持久化：

- **语法编译与并行检索**：
  - `syntax.rs`：将 GitHub 风格的查询字符串解析为抽象表达式语法树（AST）。
  - `query.rs`：将语法树编译为正则表达式、路径通配规则及 tgrep 索引查询计划。
  - `search.rs`：驱动 Rayon 线程池并发读取各仓库候选文件并执行正则短路校验。
  - `facets.rs`：在当前激活的查询过滤器下，动态统计各维度 Facet 分布及计数。
- **Trigram 索引管理**：
  - `index.rs`：负责单个仓库 tgrep 索引的打开、构建、增量更新及原子切换。
  - `watch.rs`：后台文件系统监听器，实时捕获被修改、新增或删除的脏文件集合。
- **仓库与工作区配置**：
  - `repo.rs`：仓库元数据封装、标签管理、当前分支感知与检索范围判定。
  - `library.rs`：持久化管理所有已知仓库、标签映射及各仓库专属偏好。
  - `workspace.rs`：读写 `.dowse-workspace` 工作区配置文件。
  - `session.rs`：序列化已打开的窗口、标签页与滚动状态以支持会话恢复。
  - `settings.rs`：全局设置项（如 `index.location`、`tasks.clones` 等）。
  - `config.rs`：定位系统配置目录（解析 `DOWSE_CONFIG_DIR`）。
- **Git 与 GitHub 自动化**：
  - `github.rs`：通过宿主机 `gh` CLI 查询和批量克隆 GitHub 仓库。
  - `sync.rs`：定时自动拉取逻辑与安全快进合并（Fast-forward）合法性判定。
  - `tasks.rs`：后台异步克隆与拉取任务队列及工作线程池调度。
  - `process.rs`：调用 git 与 gh 外部进程的安全封装。
- **数据排版与预览呈现**：
  - `table.rs`：将搜索结果转换为结构化数据行，提取命名正则捕获组并导出 CSV/TSV/JSON/Markdown。
  - `preview.rs`：为右侧预览窗格准备全量文件内容与匹配项高亮偏移区间。
- **自更新子系统 (`src/engine/update/`)**：
  - `release.rs`：解析 GitHub Release 的清单与发布产物元数据。
  - `client.rs`：向 GitHub API 发送更新查询请求。
  - `state.rs`：管理更新状态、版本跳过偏好持久化（`update.json`）。
  - `install.rs`：下载发行包、核对 SHA-256 签名并在本地执行原子二进制热替换。

---

### 桌面图形界面 (`src/ui/`)

基于 [GPUI Kit](https://gpui-kit.com) 构建的现代化桌面界面：

- `windows.rs`：管理桌面窗口生命周期、屏幕布局记忆与多窗口协同恢复。
- `hub.rs`：全局核心共享中枢，管理跨窗口共享的仓库句柄、文件监听器及索引队列。
- `app.rs`：单个窗口级的搜索控制器与全局快捷键分发。
- `tabs.rs`：搜索标签页容器，维持每个标签页独立的查询与滚动状态。
- `history.rs`：浏览器级的前进与后退导航历史管理器。
- `render.rs`：渲染主搜索页面、查询输入栏与检索范围选择器。
- `table.rs`：高性能虚拟化数据表格视图，支持表头排序、列宽拖拽与导出。
- `preview.rs`：只读全文件高亮预览面板，支持按 `F4` 逐个匹配点精准定位。
- `repos_page.rs` 与 `manager.rs`：仓库管理面板及其批量操作状态机。
- `tasks.rs`：后台任务看板，实时展示克隆与同步任务进度。
- `palette.rs`：支持模糊匹配的全局命令面板（Command Palette）。
- `highlight.rs`：基于 Tree-sitter 的多语言代码语法高亮管线。
- `main_menu.rs`：操作系统原生标题栏与应用菜单集成。
- `updates.rs`：新版本可用通知横幅与安装重启交互弹窗。
- `devtools.rs`：开发者诊断窗口（实时日志流、核心性能指标、查询耗时追踪）。
- `remote.rs`：接收来自外部 CLI 的 IPC 请求并桥接至界面状态。

---

### 命令行交互端 (`src/cli/`)

- `args.rs`：基于 Clap 的 CLI 命令行参数解析与子命令类型定义。
- `client.rs`：IPC 客户端逻辑，负责连接主进程 Socket 或拉起后台无头守护进程。
- `output.rs`：终端输出排版格式化：代码片段文本、流式 JSON 行、数据表格与 stderr 统计。
- `mod.rs`：CLI 入口分发调度与静态内嵌 Agent 指南（`dowse guide`）。

---

### 进程间通信 (`src/ipc/`)

- `protocol.rs`：定义 CLI 与守护进程之间通过本地 Socket 传输的强类型 JSON-RPC 数据帧。
- `mod.rs`：本地监听服务端与客户端连接管理。

---

### 系统诊断与指标 (`src/diagnostics/`)

- `log.rs`：高性能内存环形缓冲区日志器，配合 5 MB 自动轮转的磁盘日志文件。
- `metrics.rs`：性能遥测指标记录器（检索耗时分布、候选集裁剪率、内存消耗统计）。

---

### 系统平台集成与辅助模块

- `src/launch.rs`：进程无缝交接逻辑，用于自动更新后的静默重启。
- `src/bin/dowse-cli.rs`：Windows 专属控制台可执行文件入口，安装为 `dowse.com`。
- `src/shell.rs`：Windows 资源管理器右键菜单注册表交互。
- `src/editor.rs`：外部编辑器探测与跳转 URI 格式化（`code -g {file}:{line}`）。
- `src/fuzzy.rs`：命令面板专用的模糊字符串比对算法。
- `packaging/`：各平台打包元数据（macOS `Info.plist`、应用图标、Windows 资源文件）。

---

## 自动化测试策略

执行完整测试套件：

```bash
cargo test
```

- **隔离的内存与临时索引测试**：引擎单测通过 `tempfile` 在独立的系统临时目录中构建真实的 Git 仓库与 tgrep Trigram 索引，完整验证端到端检索行为且不产生任何环境副作用。
- **GPUI 界面集成测试**：位于 `src/ui/history/tests.rs` 与 `src/ui/preview/tests.rs`，利用 GPUI 提供的虚拟测试运行器验证历史导航与编辑器交互。
- **需要真机网络的测试**：两个依赖外部网络的测试用于验证与 GitHub Release 接口的真实通讯与包安装，默认跳过，需要时显式执行：
  ```bash
  cargo test -- --ignored
  ```

---

## 性能基准压测

使用专用的压测二进制分析索引构建速度与搜索吞吐：

```bash
cargo run --release --example bench -- <代码仓库路径> [查询词...]
```

该工具会精确测量全量索引耗时、增量更新耗时以及多组搜索查询的端到端延迟，输出候选集裁剪比率与吞吐数据。

---

## 自动化发布工作流

dowse 采用 GitHub Actions 实现多平台自动化编译、打包与发布：

1. 修改 `Cargo.toml` 中的版本号。
2. 提交代码并推送对应的 Git Tag：
   ```bash
   git tag v1.2.1
   git push origin v1.2.1
   ```
3. GitHub Actions 流水线（`.github/workflows/release.yml`）自动触发：
   - 在 Windows (`x86_64`)、macOS (`universal` 兼顾 Apple Silicon 与 Intel) 及 Linux (`x86_64` 与 `aarch64`) 上编译 Release 构建产物。
   - 在 Windows 上配对打包 `dowse.exe` 与 `dowse.com`。
   - 在 macOS 上为 `dowse.app` 完成 Ad-hoc 代码签名。
   - 生成各平台压缩包与对应的校验文件 `SHA256SUMS`。
   - 自动在 GitHub 创建对应 Release 并挂载发布资产。
4. 全球运行中的各平台 dowse 客户端将在每日定时检查中收到更新通知。
