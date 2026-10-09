<p align="right">
  <strong>English</strong> | <a href="zh-CN/install.md">简体中文</a>
</p>

# Installation, Updates, and Configuration

- [Pre-Built Binary Packages](#pre-built-binary-packages)
- [First-Launch System Security Approvals](#first-launch-system-security-approvals)
- [Setting up PATH](#setting-up-path)
- [Automatic and Manual Updates](#automatic-and-manual-updates)
- [Where Configuration and Data Live](#where-configuration-and-data-live)
- [Building from Source](#building-from-source)

---

## Pre-Built Binary Packages

Each GitHub [Release](https://github.com/HenryZhang-ZHY/dowse/releases/latest) provides
pre-compiled release packages verified with published `SHA256SUMS` checksums:

| Platform                              | Archive Name                          | Contents and Requirements                                                                                                                          |
| ------------------------------------- | ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Windows (x64)**                     | `dowse-<version>-windows-x86_64.zip`  | Contains `dowse.exe` (GUI application) and `dowse.com` (CLI console client). Unzip to a directory with write permissions.                          |
| **macOS 11+** (Apple Silicon / Intel) | `dowse-<version>-macos-universal.zip` | Contains universal `dowse.app`. Move to `/Applications`.                                                                                           |
| **Linux** (x64 / arm64)               | `dowse-<version>-linux-<arch>.tar.gz` | Unified binary serving as both GUI app and CLI. Requires glibc 2.35+ (Ubuntu 22.04+, Debian 12+, Fedora 36+), X11 or Wayland, and a Vulkan driver. |

---

## First-Launch System Security Approvals

Because open-source releases are not signed with a commercial certificate, your operating
system will display a security warning on initial launch:

### Windows SmartScreen

When the "Windows protected your PC" dialog appears:

1. Click **More info**.
2. Click **Run anyway**.

### macOS Gatekeeper

1. Launch `dowse.app` once (Gatekeeper will block execution).
2. Open **System Settings > Privacy & Security**, scroll to the bottom, and click **Open Anyway**.
3. Alternatively, clear the quarantine attribute set by your browser via terminal:
   ```bash
   xattr -dr com.apple.quarantine /Applications/dowse.app
   ```

### Linux Dependencies

GPUI requires Vulkan and standard windowing libraries:

- **Ubuntu/Debian**: `sudo apt install libvulkan1 libasound2 libfontconfig1`
- **Fedora**: `sudo dnf install vulkan-loader alsa-lib fontconfig`

---

## Setting up PATH

### Windows: The .exe and .com Pair

On Windows, GUI executables (`.exe`) do not attach to or print output within existing
command-line terminals. dowse solves this by providing a companion console executable,
`dowse.com`:

- When you run `dowse` in PowerShell or `cmd.exe`, the operating system prefers `.com`
  over `.exe`, invoking the console CLI.
- When you double-click or launch from Explorer, `dowse.exe` runs the desktop GUI.
- **Setup**: Place both `dowse.exe` and `dowse.com` in a shared folder (e.g. `C:\bin\dowse`)
  and add that folder to your user or system `PATH`.

### macOS

Move `dowse.app` to `/Applications`. To make the CLI accessible system-wide:

```bash
sudo ln -sf /Applications/dowse.app/Contents/MacOS/dowse /usr/local/bin/dowse
```

### Linux

Extract the archive and move the `dowse` binary to a directory on your `PATH`:

```bash
sudo install -m 755 dowse /usr/local/bin/dowse
```

---

## Automatic and Manual Updates

Once daily during runtime, dowse queries the GitHub API for newer releases:

- **Notification**: When an update is detected, a banner appears in the status bar
  displaying the new version number and release notes.
- **Verification and In-Place Swap**: Clicking **Install** downloads the matching
  platform archive, verifies its checksum against the official `SHA256SUMS`, verifies
  that the new binary executes correctly, and atomically replaces the existing executable.
- **Seamless Restart**: Clicking **Restart Now** re-opens all existing windows and
  search tabs in the updated version.

### Update Controls

- **Check Manually**: Select **Help > Check for Updates…** to query immediately.
- **Skip a Release**: Click **Skip This Version** to silence notifications for that
  release until a newer version is tagged.
- **Disable Auto-Checks**: Select **Help > Check for Updates Automatically** or run:
  ```bash
  dowse settings set updates.check false
  ```

> [!NOTE]
> Self-updates require write permissions in the directory where dowse is located. If
> installed into a read-only system path, the update prompt will instead provide a direct
> link to download the release manually. Builds compiled locally from source do not poll
> for updates.

---

## Where Configuration and Data Live

All state is preserved in your platform's standard user configuration directory:

- **Windows**: `%APPDATA%\dowse` (e.g. `C:\Users\<User>\AppData\Roaming\dowse`)
- **Linux**: `~/.config/dowse`
- **macOS**: `~/Library/Application Support/dowse`

### Directory Layout

| File / Folder    | Contents                                                                                                                                          |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `library.json`   | Inventory of all registered repositories, assigned tags, and per-repo configurations.                                                             |
| `settings.json`  | Persistent application settings (managed via `dowse settings`).                                                                                   |
| `session.json`   | Window positions, open workspaces, active search tabs, and scroll offsets for session restore.                                                    |
| `update.json`    | Cache of the latest update check results and skipped release versions.                                                                            |
| `workspaces/`    | Default storage folder for `.dowse-workspace` files.                                                                                              |
| `logs/dowse.log` | Active application log. Automatically rotates past 5 MB, archiving the previous log as `dowse.old.log`. Set `DOWSE_LOG=debug` for verbose output. |

To isolate configuration for testing or create a fully portable installation, override
the path via the `DOWSE_CONFIG_DIR` environment variable:

```bash
export DOWSE_CONFIG_DIR="/path/to/custom/dowse-config"
```

Each unique configuration directory launches its own independent background daemon.

---

## Building from Source

### Prerequisites

- A recent stable Rust toolchain (Rust 2024 edition support). The repository's
  `mise.toml` targets `latest`.

### Compilation Steps

```bash
# Clone the repository
git clone https://github.com/HenryZhang-ZHY/dowse.git
cd dowse

# Build in release mode
cargo build --release
```

The resulting binaries are placed in `target/release/`:

- **macOS / Linux**: `target/release/dowse`
- **Windows**: `target/release/dowse.exe` and `target/release/dowse-cli.exe`
  ```powershell
  # On Windows, copy dowse-cli.exe to dowse.com beside dowse.exe
  Copy-Item target\release\dowse-cli.exe target\release\dowse.com
  ```

### Quick Run

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```
