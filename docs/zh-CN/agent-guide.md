# dowse 编码助手集成指南（Agent Guide）

[English](../agent-guide.md) | <strong>简体中文</strong>

dowse 能够通过单次查询，在用户已配置的全部本地代码仓库（用户正在开发的本地工作副本、团队核心代码镜像、所依赖的第三方开源库）中执行毫秒级跨库检索。底层依赖常驻后台应用维护的 Trigram 热索引。当需要弄清当前仓库外部是如何定义、调用或配置某个接口，或者当在单库内使用常规 `grep` 过于缓慢时，使用 dowse 是最高效的选择。

## 核心心智模型

```bash
dowse search 'parse_config lang:rust'   # 检索 dowse 中已登记的所有仓库
dowse search --here 'parse_config'      # 仅检索当前工作目录所在的仓库
dowse repos                             # 查看全部仓库及其打标状态
```

所有子命令均与运行中的 dowse 进程通信。若应用当前未运行，首条命令会在后台自动启动无头守护进程并在 stderr 予以提示；该首次查询会等待索引载入内存。应用内置文件监听器，哪怕一秒钟前刚刚修改保存的文件（包括由 Agent 写入的代码），都会立刻被检索命中。

搜索结果输出至 stdout；命中计数、检索耗时、收窄建议与运行诊断信息输出至 stderr。退出状态码：`0` 代表找到匹配，`1` 代表未找到匹配，`2` 代表执行出错。

## 查询语法

采用 GitHub 代码搜索语法规范。各项条件以**文件**为粒度组合，而非以单行组合。

| 查询语法                                     | 匹配行为                                                  |
| -------------------------------------------- | --------------------------------------------------------- |
| `parse config`                               | 检索同时包含两个词项的文件；两者的命中行均会展示          |
| `parse OR config`                            | 检索包含任意一个词项的文件                                |
| `parse NOT test`                             | 检索包含 `parse` 但不包含 `test` 的文件                   |
| `"fn main()"`                                | 精确短语匹配（在 Shell 中注意加引号转义）                 |
| `/fn \w+_test/`                              | 逐行匹配正则表达式                                        |
| `path:src/*.rs`, `path:engine`               | 路径 Glob 通配（包含 `/` 则从根路径锚定），或路径文本匹配 |
| `language:rust`, `lang:ts`                   | 按编程语言限定                                            |
| `repo:api`, `branch:main`, `tag:owner:alice` | 在满足条件的仓库中检索                                    |
| `-path:tests`, `-lang:md`                    | 排除匹配该限定符的文件                                    |

默认忽略大小写，传入 `-s` 开启大小写敏感。`-w` 开启全字匹配。`-r` 将整条查询视为单条正则表达式，适用于从别处直接粘贴复杂正则。

## 控制输出体积与 Token 预算

- 默认输出上限为 100 条匹配行（可通过 `-n` 修改，`-n 0` 移除上限），单文件最多 20 条（可通过 `-m` 修改）。stderr 底部会输出匹配总数。
- 当结果因超额被截断时，底部会基于 Facet 分布提供收窄建议，并附带各自保留的文件数：
  `narrow with: repo:api (120) language:Rust (80) path:src/** (64)`。
  可直接将建议中的限定符追加至查询末尾。
- `-l` 仅打印包含命中的相对文件路径；`-c` 打印路径及命中计数。建议先用 `-l` 宏观粗筛，再针对核心文件展开查阅。
- `-C 2` 在需要阅读上下文时，为每处匹配附带前后 2 行代码。
- `-q` 静默模式，不输出任何文本，直接依据退出状态码判断。

## 检索范围限定（Scope）

若不显式指定范围参数，查询将覆盖 dowse 已登记的全部仓库。

- `--here`：仅检索包含当前工作目录的本地仓库。
- `-t TAG`（可重复）：筛选具备该标签的仓库。同一组内的标签（如 `owner:alice`、`owner:bob`）为 OR 关系；跨组标签（如 `-t dev -t owner:alice`）为 AND 关系。每个仓库同时具备 `branch:<分支名>` 标签。
- `-W NAME`：仅检索已保存工作区文件包含的仓库。
- 在查询语句中直接写入 `repo:`、`branch:` 与 `tag:` 亦可针对单次查询完成过滤。

执行 `dowse repos` 可查看所有仓库的分支、索引状态、文件总数、更新时间与标签；追加 `--json` 可获得结构化输出。

## 面向机器的结构化输出

`--json` 以单行 JSON（NDJSON）流式输出：每命中一个文件输出一行 `file` 对象，流末尾输出一行 `summary` 统计摘要。

```json
{"type":"file","repo":"api","branch":"main","path":"src/config.rs","abs_path":"/src/api/src/config.rs","language":"Rust","matched_lines":2,"lines":[{"line":12,"text":"pub fn parse_config(","match":true,"ranges":[[7,19]]}]}
{"type":"summary","matched_lines":2,"files":1,"shown_lines":2,"shown_files":1,"searched_files":3,"corpus_files":4120,"repos":5,"unindexed_repos":0,"truncated":false,"elapsed_ms":8.1,"candidates_ms":0.9,"bytes_read":20480,"facets":[]}
```

`--table csv|tsv|md|json` 将每条匹配行输出为结构化表格行，包含仓库、分支、路径、行号、列号、语言、命中片段与整行文本。如果正则查询中包含命名捕获组，每个捕获组会自动形成独立的数据列，从而将代码搜索直接转化为数据提取流程：

```bash
dowse search --table csv '/version = "(?<version>[^"]+)"/ path:Cargo.toml'
```

## 仓库管理常用命令

```bash
dowse repos add ~/src/api ~/src/web -t dev   # 批量添加仓库并打上 dev 标签
dowse repos add ~/mirrors                    # 传入父目录，自动递归添加其下全部仓库
dowse repos tag api owner:alice -r mirror    # 为 api 仓库添加 owner:alice，移除 mirror
dowse index --here --wait                    # 增量更新当前仓库索引并阻塞等待完成
dowse index --here --full                    # 全量重编当前仓库索引
dowse repos index-location api              # 查看该仓库索引存放路径
dowse repos index-location api --external   # 将索引迁移至仓库外部目录
dowse settings                               # 查看全局设置
dowse status                                 # 查看应用、窗口与索引健康度
```

通过 GitHub 获取代码（复用宿主机的 `gh` 登录态）：

```bash
dowse repos github my-org -q                 # 列出该组织的全部仓库，仅输出名称
dowse repos clone my-org/api --into ~/mirrors --wait   # 克隆、注册并等待完成
dowse repos clone --from my-org --pull-every 1h        # 批量克隆全部仓库并配置定时同步
dowse repos pull api --wait                  # 抓取并仅在安全时执行快进合并
dowse tasks                                  # 查看后台克隆与拉取任务队列
```

克隆默认存放于 `<目录>/<所有者>/<仓库名>`，自动打上 `owner:<所有者>` 标签，且默认采用 Blobless 模式（保留完整历史，正文按需懒加载）。定时拉取绝不合并冲突或修改本地工作树：当处于非默认分支、有未提交修改或有本地提交时，仅执行 fetch 并说明跳过原因。

尚未建好索引的仓库在后台构建期间会自动退化为常规遍历扫描，底部摘要会对此做出说明。

## 异常排查

执行 `dowse dev logs` 可查看应用最新日志记录（追加 `--level debug` 查看调试详情，`-f` 跟踪实时日志流）。执行 `dowse status` 可获取物理日志文件的完整磁盘路径。
