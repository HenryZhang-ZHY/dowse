<sub><a href="../install.md">English</a> · <strong>简体中文</strong></sub>

# 安装、更新与环境配置

- [预编译安装包下载](#预编译安装包下载)
- [初次启动系统安全提示处理](#初次启动系统安全提示处理)
- [环境变量 PATH 配置](#环境变量-path-配置)
- [自动与手动更新](#自动与手动更新)
- [配置文件与数据存放位置](#配置文件与数据存放位置)
- [从源码编译构建](#从源码编译构建)

---

## 预编译安装包下载

前往 [GitHub Releases](https://github.com/HenryZhang-ZHY/dowse/releases/latest) 下载对应平台的预编译安装包，官方发布包均附带 `SHA256SUMS` 校验文件：

| 平台                                  | 压缩包文件名                          | 内容包含与运行要求                                                                                                                   |
| ------------------------------------- | ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| **Windows (x64)**                     | `dowse-<version>-windows-x86_64.zip`  | 包含 `dowse.exe`（GUI 主程序）与 `dowse.com`（CLI 控制台入口）。解压至拥有写权限的目录即可。                                         |
| **macOS 11+** (Apple Silicon / Intel) | `dowse-<version>-macos-universal.zip` | 通用二进制格式 `dowse.app`。解压后直接拖入 `/Applications` 应用程序目录。                                                            |
| **Linux** (x64 / arm64)               | `dowse-<version>-linux-<arch>.tar.gz` | 单一二进制程序，兼具 GUI 与 CLI 功能。需要 glibc 2.35+（Ubuntu 22.04+、Debian 12+、Fedora 36+）、X11 或 Wayland 及 Vulkan 图形驱动。 |

---

## 初次启动系统安全提示处理

由于开源构建未购买商业代码签名证书，初次启动时操作系统可能会弹出安全拦截提示：

### Windows SmartScreen

当弹出“Windows 已保护你的电脑”提示框时：

1. 点击左上方的 **更多信息**（More info）。
2. 点击右下方的 **仍要运行**（Run anyway）。

### macOS Gatekeeper

1. 首次打开 `dowse.app` 时系统可能会拦截并提示未知开发者。
2. 打开系统 **设置 > 隐私与安全性**，滑至页面底部，在安全性分栏中点击 **仍要打开**（Open Anyway）。
3. 或者直接在终端中清除浏览器下载时附带的隔离属性：
   ```bash
   xattr -dr com.apple.quarantine /Applications/dowse.app
   ```

### Linux 依赖项

GPUI 底层依赖 Vulkan 及基础窗口渲染库：

- **Ubuntu/Debian**：`sudo apt install libvulkan1 libasound2 libfontconfig1`
- **Fedora**：`sudo dnf install vulkan-loader alsa-lib fontconfig`

---

## 环境变量 PATH 配置

### Windows：.exe 与 .com 协作机制

在 Windows 上，常规 GUI 应用程序（`.exe`）启动时默认脱离当前控制台，无法直接在终端中输出内容。为此，dowse 提供了配套的控制台入口程序 `dowse.com`：

- 当在 PowerShell 或 `cmd.exe` 中输入 `dowse` 时，Windows 系统会根据扩展名优先级优先执行 `.com` 文件，在当前终端中运行 CLI。
- 当在文件资源管理器中双击或通过快捷方式打开时，`dowse.exe` 会启动 GPUI 桌面图形界面。
- **环境配置**：将解压得到的 `dowse.exe` 与 `dowse.com` 放置在同一目录（如 `C:\bin\dowse`），并将该目录添加至系统或用户环境变量 `PATH`。

### macOS

将 `dowse.app` 移动至 `/Applications` 目录。若要在全局终端中调用 CLI 命令，创建软链接即可：

```bash
sudo ln -sf /Applications/dowse.app/Contents/MacOS/dowse /usr/local/bin/dowse
```

### Linux

解压压缩包后，将 `dowse` 可执行文件安装至系统的 `PATH` 目录：

```bash
sudo install -m 755 dowse /usr/local/bin/dowse
```

---

## 自动与手动更新

dowse 在运行期间每日会自动向 GitHub 查询一次最新发布版本：

- **更新提示**：检测到新版本时，应用状态栏右侧会显示更新横幅，展示新版本号及更新日志（Release Notes）。
- **校验与就地替换**：点击 **Install** 会自动下载当前平台对应的安装包，核对官方发布的 `SHA256SUMS` 校验和，并在验证新程序可正常运行后执行就地原子替换。
- **平滑重启**：点击 **Restart Now** 可立即启动新版本，并自动恢复之前打开的所有窗口与搜索标签页。

### 更新配置项

- **手动检查更新**：点击菜单栏 **Help > Check for Updates…** 即可立即检查。
- **跳过特定版本**：点击 **Skip This Version** 可屏蔽该版本的更新提示，直到下一个新版本发布。
- **关闭每日自动检测**：点击菜单栏 **Help > Check for Updates Automatically** 或在终端执行：
  ```bash
  dowse settings set updates.check false
  ```

> [!NOTE]
> 自动更新要求 dowse 所在目录拥有写权限。若程序安装在管理员只读路径下，更新提示将自动退化为提供手动下载链接。本地从源码自行编译的版本不会触发自动更新检查。

---

## 配置文件与数据存放位置

所有用户设置、工作区配置与运行状态数据均保存在操作系统标准的用户配置目录中：

- **Windows**：`%APPDATA%\dowse`（如 `C:\Users\<用户名>\AppData\Roaming\dowse`）
- **Linux**：`~/.config/dowse`
- **macOS**：`~/Library/Application Support/dowse`

### 目录结构一览

| 文件 / 文件夹    | 存储内容说明                                                                                                      |
| ---------------- | ----------------------------------------------------------------------------------------------------------------- |
| `library.json`   | 所有已注册仓库清单、各自分配的标签及单库配置。                                                                    |
| `settings.json`  | 全局持久化设置（通过 `dowse settings` 读取与修改）。                                                              |
| `session.json`   | 窗口坐标、已打开的工作区、活跃标签页与滚动偏移量，用于会话自动恢复。                                              |
| `update.json`    | 最近一次更新检测结果缓存及已跳过的版本号记录。                                                                    |
| `workspaces/`    | 默认存放 `.dowse-workspace` 工作区文件的目录。                                                                    |
| `logs/dowse.log` | 应用活动日志。超过 5 MB 自动轮转并备份为 `dowse.old.log`。设置环境变量 `DOWSE_LOG=debug` 可开启更详尽的诊断追踪。 |

若需要隔离测试环境或制作免安装绿色便携版，可配置环境变量 `DOWSE_CONFIG_DIR`：

```bash
export DOWSE_CONFIG_DIR="/path/to/custom/dowse-config"
```

每个独立的配置目录都会维护各自专属的后台单实例守护进程。

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
