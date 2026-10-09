<p align="right">
  <a href="../install.md">English</a> | <strong>简体中文</strong>
</p>

# 安装、更新与环境配置

- [预编译安装包下载](#预编译安装包下载)
- [初次启动系统安全提示处理](#初次启动系统安全提示处理)
- [环境变量 PATH 配置](#环境变量-path-配置)
- [自动与手动更新](#自动与手动更新)
- [配置文件与数据存放位置](#配置文件与数据存放位置)
- [从源码编译构建](#从源码编译构建)

---

## 预编译安装包下载

前往 [GitHub Releases](https://github.com/HenryZhang-ZHY/dowse/releases/latest) 下载对应平台的免编译发行压缩包，官方发布包均附带 `SHA256SUMS` 校验值：

| 平台                                  | 压缩包文件名                          | 内容包含与运行要求                                                                                                                   |
| ------------------------------------- | ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| **Windows (x64)**                     | `dowse-<version>-windows-x86_64.zip`  | 包含 `dowse.exe`（GUI 主程序）与 `dowse.com`（CLI 控制台入口）。解压至具备写权限的目录即可。                                         |
| **macOS 11+** (Apple Silicon / Intel) | `dowse-<version>-macos-universal.zip` | 通用二进制格式 `dowse.app`。解压后直接拖入 `/Applications` 应用程序目录。                                                            |
| **Linux** (x64 / arm64)               | `dowse-<version>-linux-<arch>.tar.gz` | 单一可执行程序，兼具 GUI 与 CLI 功能。需要 glibc 2.35+（Ubuntu 22.04+、Debian 12+、Fedora 36+）、X11 或 Wayland 及 Vulkan 图形驱动。 |

---

## 初次启动系统安全提示处理

开源构建未购买商业机构代码签名证书，各操作系统在首次运行时可能弹出安全拦截：

### Windows SmartScreen

当弹出“Windows 已保护你的电脑”提示框时：

1. 点击左上方的 **更多信息**（More info）。
2. 点击右下方的 **仍要运行**（Run anyway）。

### macOS Gatekeeper

1. 首次打开 `dowse.app` 时系统会拦截并提示未知开发者。
2. 打开系统 **设置 > 隐私与安全性**，滑至页面底部，在安全性分栏中点击 **仍要打开**（Open Anyway）。
3. 或者直接在终端中清除浏览器下载时打上的隔离属性：
   ```bash
   xattr -dr com.apple.quarantine /Applications/dowse.app
   ```

### Linux 依赖项

GPUI 底层依赖 Vulkan 及常规窗口渲染库：

- **Ubuntu/Debian**：`sudo apt install libvulkan1 libasound2 libfontconfig1`
- **Fedora**：`sudo dnf install vulkan-loader alsa-lib fontconfig`

---

## 环境变量 PATH 配置

### Windows：.exe 与 .com 双进程配合

在 Windows 系统中，常规 GUI 应用程序（`.exe`）启动时默认脱离终端，既不阻塞终端也不会将日志输出打印到控制台中。dowse 巧妙地通过配套的控制台入口 `dowse.com` 解决了这一问题：

- 当你在 PowerShell 或 `cmd.exe` 中输入 `dowse` 时，Windows 系统会根据可执行后缀优先级优先调用 `.com` 文件，启动 CLI 控制台逻辑。
- 当你在资源管理器中双击或快捷方式打开时，`dowse.exe` 会启动 GPUI 桌面图形界面。
- **配置方法**：将解压出来的 `dowse.exe` 与 `dowse.com` 放置在同一文件夹（如 `C:\bin\dowse`），并将该路径加入系统或用户环境变量 `PATH`。

### macOS

将 `dowse.app` 移动至 `/Applications`。若要在全局终端中调用 CLI 命令，建立软链接即可：

```bash
sudo ln -sf /Applications/dowse.app/Contents/MacOS/dowse /usr/local/bin/dowse
```

### Linux

解压压缩包后，将 `dowse` 二进制程序放入系统 `PATH` 目录：

```bash
sudo install -m 755 dowse /usr/local/bin/dowse
```

---

## 自动与手动更新

dowse 在运行期间每天会静默向 GitHub 检查一次最新版本：

- **更新提示**：若发现新版本，应用状态栏右侧会出现更新横幅，展示新版本号及 Release Notes。
- **校验与就地热替换**：点击 **Install** 会自动下载当前平台对应的压缩包，核对其官方 `SHA256SUMS` 哈希值，启动新程序自检成功后，在本地目录进行原子级就地替换。
- **无缝重启**：点击 **Restart Now** 可立即使用新版本重新唤起当前已打开的全部窗口与搜索标签页。

### 更新配置项

- **手动立即检查**：点击菜单栏 **Help > Check for Updates…** 即可立即检查。
- **跳过特定版本**：点击 **Skip This Version** 可屏蔽该版本的更新提示，直到下一个更新版本发布。
- **关闭每日自动检测**：点击菜单栏 **Help > Check for Updates Automatically** 或在终端执行：
  ```bash
  dowse settings set updates.check false
  ```

> [!NOTE]
> 自动更新功能要求 dowse 所在目录具备写权限。若程序被安装在管理员只读路径中，更新弹窗会自动降级为提供官方下载链接。本地从源码自编译的版本不会向 GitHub 查询自动更新。

---

## 配置文件与数据存放位置

所有用户偏好、工作区与状态数据均统一存放在系统的标准配置目录中：

- **Windows**：`%APPDATA%\dowse`（如 `C:\Users\<用户名>\AppData\Roaming\dowse`）
- **Linux**：`~/.config/dowse`
- **macOS**：`~/Library/Application Support/dowse`

### 目录结构一览

| 文件 / 文件夹    | 存储内容说明                                                                                                      |
| ---------------- | ----------------------------------------------------------------------------------------------------------------- |
| `library.json`   | 所有已注册仓库清单、各自分配的标签及单库配置。                                                                    |
| `settings.json`  | 全局持久化设置（通过 `dowse settings` 读取与修改）。                                                              |
| `session.json`   | 窗口坐标、已打开的工作区、活跃标签页与滚动偏移量，用于会话自动恢复。                                              |
| `update.json`    | 最近一次更新探测结果缓存及被用户跳过的版本号记录。                                                                |
| `workspaces/`    | 默认存放 `.dowse-workspace` 工作区文件的目录。                                                                    |
| `logs/dowse.log` | 应用活动日志。超过 5 MB 自动滚动并备份为 `dowse.old.log`。设置环境变量 `DOWSE_LOG=debug` 可开启更详尽的诊断追踪。 |

若需要隔离测试环境或制作免安装绿色便携版，可配置环境变量 `DOWSE_CONFIG_DIR`：

```bash
export DOWSE_CONFIG_DIR="/path/to/custom/dowse-config"
```

每个独立的配置目录均会维持各自专属的单实例后台服务。

---

## 从源码编译构建

### 环境要求

- 较新的稳定版 Rust 工具链（支持 Rust 2024 Edition）。仓库中的 `mise.toml` 锁定为 `latest`。

### 编译步骤

```bash
# 克隆代码仓库
git clone https://github.com/HenryZhang-ZHY/dowse.git
cd dowse

# 以 Release 模式编译
cargo build --release
```

编译产物位于 `target/release/` 目录下：

- **macOS / Linux**：`target/release/dowse`
- **Windows**：`target/release/dowse.exe` 与 `target/release/dowse-cli.exe`
  ```powershell
  # 在 Windows 上，将 dowse-cli.exe 拷贝至同目录下重命名为 dowse.com
  Copy-Item target\release\dowse-cli.exe target\release\dowse.com
  ```

### 直接运行

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```
