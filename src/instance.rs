//! One running app per settings directory, as with `code`: a later launch
//! hands its command line to the running app over a local socket (a named
//! pipe on Windows) and exits.

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::Path;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};

use crate::cli::Command;

/// What this launch does.
pub enum Launch {
    /// This process runs the app; later launches' commands arrive here.
    Primary(async_channel::Receiver<Command>),
    /// The running app took the command; this process is done.
    Forwarded,
}

/// Hand `command` to the app already running with the settings in
/// `config_root`, or become that app.
pub fn claim(config_root: &Path, command: &Command) -> Launch {
    claim_named(&socket_id(config_root), command)
}

fn claim_named(id: &str, command: &Command) -> Launch {
    // Two tries: when another launch creates the socket between our attempt
    // to connect and our attempt to listen, hand over to it instead.
    for _ in 0..2 {
        if forward(id, command).is_ok() {
            return Launch::Forwarded;
        }
        match listen(id) {
            Ok(commands) => return Launch::Primary(commands),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(_) => break,
        }
    }
    // Without a socket, run on our own rather than not at all.
    let (_, commands) = async_channel::unbounded();
    Launch::Primary(commands)
}

/// Send `command` to the running app and wait until it has it.
fn forward(id: &str, command: &Command) -> io::Result<()> {
    let mut stream = BufReader::new(Stream::connect(socket_name(id)?)?);
    allow_running_app_to_take_focus();
    let mut line = serde_json::to_string(command).map_err(io::Error::other)?;
    line.push('\n');
    stream.get_mut().write_all(line.as_bytes())?;
    let mut reply = String::new();
    stream.read_line(&mut reply)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(io::Error::other("the running app did not take the command"))
    }
}

/// Listen for later launches on a background thread.
fn listen(id: &str) -> io::Result<async_channel::Receiver<Command>> {
    let listener = ListenerOptions::new()
        .name(socket_name(id)?)
        // A socket file left by a crash on Unix; named pipes vanish with
        // their process. Only reached when nobody answered on it.
        .try_overwrite(true)
        .create_sync()?;
    let (sender, receiver) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("dowse instance".into())
        .spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                if stream.read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(command) = serde_json::from_str::<Command>(&line) else {
                    continue;
                };
                if sender.send_blocking(command).is_err() {
                    break;
                }
                stream.get_mut().write_all(b"ok\n").ok();
            }
        })?;
    Ok(receiver)
}

/// A socket name per user and settings directory, so that a portable
/// install or a test run with its own settings gets its own app.
fn socket_id(config_root: &Path) -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let root = config_root.to_string_lossy().to_lowercase();
    format!(
        "dowse-{:016x}.sock",
        fnv1a(format!("{user}\n{root}").as_bytes())
    )
}

fn socket_name(id: &str) -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        id.to_string().to_ns_name::<GenericNamespaced>()
    } else {
        std::env::temp_dir()
            .join(id)
            .to_fs_name::<GenericFilePath>()
    }
}

/// A hash that stays the same across builds, unlike std's.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Windows only lets the foreground process bring a window forward. The user
/// just started this one, so it may pass that right to the running app.
fn allow_running_app_to_take_focus() {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn AllowSetForegroundWindow(process_id: u32) -> i32;
        }
        const ASFW_ANY: u32 = u32::MAX;
        // SAFETY: no pointers; failure only means the window stays behind.
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_second_launch_hands_its_command_to_the_first() {
        let id = format!("dowse-test-{}.sock", std::process::id());
        let Launch::Primary(commands) = claim_named(&id, &Command::Start) else {
            panic!("the first launch should run the app");
        };
        let add = Command::Add(vec![PathBuf::from("/src/api")]);
        assert!(matches!(claim_named(&id, &add), Launch::Forwarded));
        assert_eq!(commands.recv_blocking().unwrap(), add);
    }

    #[test]
    fn socket_ids_differ_by_settings_directory() {
        let a = socket_id(Path::new("/config/a"));
        assert_eq!(a, socket_id(Path::new("/CONFIG/A")));
        assert_ne!(a, socket_id(Path::new("/config/b")));
    }
}
