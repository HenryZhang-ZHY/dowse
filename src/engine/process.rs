//! Running `git` and `gh` from the app: without flashing a console on
//! Windows, cancellable, and reading the progress git writes to stderr.

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

/// A command for `program` that shows no console window of its own.
pub fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `git` run in `root`.
pub fn git(root: &Path) -> Command {
    let mut git = command("git");
    git.arg("-C").arg(root);
    // Never wait for a password prompt nobody can see.
    git.env("GIT_TERMINAL_PROMPT", "0");
    git
}

/// Run `command` to completion, failing with its stderr when it fails.
pub fn output(mut command: Command) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .with_context(|| format!("cannot run {program}; is it installed and on PATH?"))?;
    if !output.status.success() {
        bail!("{}", failure(&program, &output.stderr));
    }
    Ok(output)
}

/// Run `command` and return its trimmed stdout.
pub fn stdout(command: Command) -> Result<String> {
    let output = output(command)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `command` to completion like [`output`], but stop it (and what it
/// started) as soon as `cancelled` says so, failing with "cancelled".
pub fn output_until(mut command: Command, cancelled: &dyn Fn() -> bool) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {program}; is it installed and on PATH?"))?;
    let read_all = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).ok();
            bytes
        })
    };
    let stdout = read_all(Box::new(child.stdout.take().expect("stdout is piped")));
    let stderr = read_all(Box::new(child.stderr.take().expect("stderr is piped")));
    let status = loop {
        if cancelled() {
            kill_tree(&mut child);
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let output = Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    };
    if !output.status.success() {
        bail!("{}", failure(&program, &output.stderr));
    }
    Ok(output)
}

/// Run `command`, passing each line it writes to stderr to `on_line` (git
/// ends progress lines with `\r`), and stop it when `cancel` is set. Fails
/// with the last lines of stderr when it fails.
pub fn run_with_progress(
    mut command: Command,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {program}; is it installed and on PATH?"))?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let (lines, received) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut line = Vec::new();
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            for &byte in &buffer[..read] {
                if byte == b'\r' || byte == b'\n' {
                    if !line.is_empty() {
                        let text = String::from_utf8_lossy(&line).into_owned();
                        if lines.send(text).is_err() {
                            return;
                        }
                        line.clear();
                    }
                } else {
                    line.push(byte);
                }
            }
        }
        if !line.is_empty() {
            lines.send(String::from_utf8_lossy(&line).into_owned()).ok();
        }
    });

    let mut tail: Vec<String> = Vec::new();
    let status = loop {
        while let Ok(line) = received.try_recv() {
            on_line(&line);
            tail.push(line);
            if tail.len() > 20 {
                tail.remove(0);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            reader.join().ok();
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    reader.join().ok();
    for line in received.try_iter() {
        on_line(&line);
        tail.push(line);
    }
    if !status.success() {
        let text = tail.join("\n");
        bail!("{}", failure(&program, text.as_bytes()));
    }
    Ok(())
}

/// Stop `child` and whatever it started: `gh` runs `git`, which runs a
/// remote helper, and on Windows killing a parent leaves its children.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut taskkill = command("taskkill");
        taskkill
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if taskkill.status().is_ok_and(|status| status.success()) {
            child.wait().ok();
            return;
        }
    }
    child.kill().ok();
    child.wait().ok();
}

/// The useful part of a failed command's stderr: its `fatal:`/`error:` lines
/// when there are any, otherwise its last line that is not progress.
fn failure(program: &str, stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.starts_with("fatal:") || lower.starts_with("error:")
        })
        .collect();
    let message = if errors.is_empty() {
        lines
            .iter()
            .rev()
            .find(|line| !line.contains('%'))
            .copied()
            .unwrap_or("")
            .to_string()
    } else {
        errors.join("; ")
    };
    if message.is_empty() {
        format!("{program} failed")
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_prefer_fatal_and_error_lines() {
        let stderr =
            b"Cloning into 'x'...\nReceiving objects:  10% (1/10)\nfatal: repository not found\n";
        assert_eq!(failure("git", stderr), "fatal: repository not found");
        assert_eq!(
            failure("gh", b"Receiving objects: 10%\nsomething odd\n"),
            "something odd"
        );
        assert_eq!(failure("gh", b""), "gh failed");
    }

    #[test]
    fn output_until_returns_output_or_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let mut version = git(dir.path());
        version.arg("--version");
        let output = output_until(version, &|| false).unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version"));

        let mut bad = git(dir.path());
        bad.arg("log");
        let error = output_until(bad, &|| false).unwrap_err().to_string();
        assert!(error.starts_with("fatal:"), "{error}");

        let started = std::time::Instant::now();
        let mut slow = git(dir.path());
        slow.arg("--version");
        let error = output_until(slow, &|| true).unwrap_err().to_string();
        assert_eq!(error, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn reports_progress_lines_and_the_fatal_line_of_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.txt"), "hello").unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["add", "."],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "x",
            ],
        ] {
            let mut git = git(&source);
            git.args(args);
            output(git).unwrap();
        }
        let path = source.display().to_string().replace('\\', "/");
        let url = format!("file:///{}", path.trim_start_matches('/'));
        let cancel = AtomicBool::new(false);
        let mut lines = Vec::new();
        let mut clone = git(dir.path());
        clone.args(["clone", "--progress", &url, "copy"]);
        run_with_progress(clone, &cancel, |line| lines.push(line.to_string())).unwrap();
        assert!(dir.path().join("copy/a.txt").exists());
        assert!(
            lines.iter().any(|line| line.contains("objects")),
            "{lines:?}"
        );

        let mut again = git(dir.path());
        again.args(["clone", "--progress", &url, "copy"]);
        let error = run_with_progress(again, &cancel, |_| {})
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("fatal:"), "{error}");

        cancel.store(true, Ordering::Relaxed);
        let mut cancelled = git(dir.path());
        cancelled.args(["clone", "--progress", &url, "other"]);
        let error = run_with_progress(cancelled, &cancel, |_| {})
            .unwrap_err()
            .to_string();
        assert_eq!(error, "cancelled");
    }
}
