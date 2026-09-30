# Install and update

## Download

Each [release](https://github.com/HenryZhang-ZHY/dowse/releases/latest) has builds
ready to run, with their checksums in `SHA256SUMS`:

| Platform | Archive | What's in it |
| --- | --- | --- |
| Windows (x64) | `dowse-<version>-windows-x86_64.zip` | `dowse.exe`, the app, and `dowse.com`, the command line (see [below](#windows-the-app-and-the-command-line)). Unzip them to a folder, and add it to `PATH` to run `dowse` in a terminal. |
| macOS 11 or later (Apple silicon and Intel) | `dowse-<version>-macos-universal.zip` | `dowse.app`. Move it to Applications. For the command line, link `/Applications/dowse.app/Contents/MacOS/dowse` into a folder on `PATH`. |
| Linux (x64, arm64) | `dowse-<version>-linux-<arch>.tar.gz` | `dowse`, both the app and the command line. Needs glibc 2.35 or later (Ubuntu 22.04, Debian 12, Fedora 36), X11 or Wayland, and a Vulkan driver. |

## The first start

The builds are not signed with a paid certificate, so the system asks once before
the first start. On Windows, SmartScreen's "Windows protected your PC" has a
**More info** link, then **Run anyway**. On macOS, open `dowse.app` once, then allow
it under System Settings > Privacy & Security > **Open Anyway**, or clear the
quarantine flag the browser set:

```bash
xattr -dr com.apple.quarantine /Applications/dowse.app
```

## Updates

Once a day while it runs, dowse asks GitHub for its latest release; a newer one
shows in the status bar with its release notes. Install downloads this platform's
archive, checks it against the SHA-256 GitHub keeps for it, makes sure the new
program starts, and puts it in place of the running one; Restart Now brings the
same windows back in the new version.

- Help > Check for Updates… asks at once.
- Skip This Version stops offering one release; the next one is offered again.
- Help > Check for Updates Automatically, or `dowse settings set updates.check false`,
  turns the daily question off.

An update replaces the programs where they are, so keep them in a folder you can
write to. When dowse cannot write there (Program Files, say), or macOS runs
`dowse.app` from a read-only copy because it was never moved to Applications, the
dialog says so and links the release page instead. Only the builds released on
GitHub look for updates on their own; one built from source does not.

## Build from source

Requires a recent stable Rust (the repository's `mise.toml` pins `latest`).

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```

On Linux, GPUI needs the usual X11/Wayland and Vulkan development packages; see the
[Zed Linux build notes](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

### Windows: the app and the command line

On Windows the app is a GUI program, which a terminal neither waits for nor shows
output from. Install the console program Cargo builds as `dowse-cli.exe` next to
it as `dowse.com`; terminals prefer `.com` to `.exe`, so `dowse` in a terminal runs
the command line, which hands launches to `dowse.exe`, while Explorer runs the app:

```powershell
cargo build --release
Copy-Item target\release\dowse-cli.exe target\release\dowse.com
```

On macOS and Linux the `dowse` binary is both.

## Where settings live

Settings live under the user configuration directory (`%APPDATA%\dowse` on
Windows, `~/.config/dowse` on Linux, `~/Library/Application Support/dowse` on
macOS):

| File | What it holds |
| --- | --- |
| `library.json` | Every repository's name, tags and settings. |
| `settings.json` | The app's settings; `dowse settings` lists them. |
| `session.json` | The windows to restore and recent workspaces. |
| `update.json` | What the last look for updates found. |
| `workspaces/` | Where workspaces are saved unless you pick elsewhere. |
| `logs/dowse.log` | The log. It starts over past 5 MB, keeping the previous one as `dowse.old.log`; `DOWSE_LOG=debug` records more. |

Set `DOWSE_CONFIG_DIR` to keep settings elsewhere, for a portable install or a test.
Each settings directory gets its own running app.
