//! How the command line and the running app talk. As with `code`, one app
//! runs per settings directory: a later launch hands its command line to the
//! running app over a local socket (a named pipe on Windows) and exits. The
//! `dowse` subcommands send their requests the same way and read the
//! answers back; see [`protocol`].

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};

use crate::launch::Command;
use protocol::{ClientMessage, Frame, PROTOCOL_VERSION, Request, RequestEnvelope};

pub mod protocol;

/// What this launch does.
pub enum Launch {
    /// This process runs the app; later launches and command-line requests
    /// arrive here.
    Primary(async_channel::Receiver<Incoming>),
    /// The running app took the command; this process is done.
    Forwarded,
}

/// What reaches the running app over the socket.
pub enum Incoming {
    /// A later launch's command line.
    Launch(Command),
    /// A command-line request. Send the answer to the sender, ending with
    /// [`Frame::Done`] or [`Frame::Error`]; a failed send means the command
    /// line went away.
    Request(RequestEnvelope, async_channel::Sender<Frame>),
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
            Ok(incoming) => return Launch::Primary(incoming),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(_) => break,
        }
    }
    // Without a socket, run on our own rather than not at all.
    let (_, incoming) = async_channel::unbounded();
    Launch::Primary(incoming)
}

/// Wait until no app runs with the settings in `config_root`, for at most
/// `timeout`, as an app started to take over from one that is quitting
/// must: were it to claim the socket first, it would hand its command to
/// the app on its way out. Returns whether that app is gone.
pub fn wait_until_gone(config_root: &Path, timeout: Duration) -> bool {
    wait_until_named_gone(&socket_id(config_root), timeout)
}

fn wait_until_named_gone(id: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        // The app reads nothing from the connection, and lets it go.
        if Connection::open(id).is_err() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Send `command` to the running app and wait until it has it.
fn forward(id: &str, command: &Command) -> io::Result<()> {
    let connection = Connection::open(id)?;
    allow_running_app_to_take_focus();
    for frame in connection.send(&ClientMessage::Launch(command.clone()))? {
        match frame? {
            Frame::Done => return Ok(()),
            Frame::Error(error) => return Err(io::Error::other(error)),
            _ => {}
        }
    }
    Err(io::Error::other("the running app did not take the command"))
}

/// A connection to the running app.
pub struct Connection {
    stream: BufReader<Stream>,
}

impl Connection {
    /// Connect to the app running with the settings in `config_root`. Fails
    /// when none runs.
    pub fn connect(config_root: &Path) -> io::Result<Self> {
        Self::open(&socket_id(config_root))
    }

    fn open(id: &str) -> io::Result<Self> {
        Ok(Self {
            stream: BufReader::new(Stream::connect(socket_name(id)?)?),
        })
    }

    /// Send a request and read the answer.
    pub fn request(self, request: Request, cwd: &Path) -> io::Result<Frames> {
        self.send(&ClientMessage::Request(RequestEnvelope {
            version: PROTOCOL_VERSION,
            cwd: cwd.to_path_buf(),
            request,
        }))
    }

    fn send(mut self, message: &ClientMessage) -> io::Result<Frames> {
        write_line(self.stream.get_mut(), message)?;
        Ok(Frames {
            stream: self.stream,
            finished: false,
        })
    }
}

/// The frames of an answer, up to and including the last one.
pub struct Frames {
    stream: BufReader<Stream>,
    finished: bool,
}

impl Iterator for Frames {
    type Item = io::Result<Frame>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let mut line = String::new();
        let frame = match self.stream.read_line(&mut line) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the running app closed the connection before answering; \
                 if dowse was just updated, quit it and try again",
            )),
            Ok(_) => serde_json::from_str::<Frame>(&line).map_err(io::Error::other),
            Err(error) => Err(error),
        };
        self.finished = frame.as_ref().map_or(true, Frame::is_last);
        Some(frame)
    }
}

impl Frame {
    /// Whether nothing follows this frame.
    pub fn is_last(&self) -> bool {
        matches!(self, Self::Done | Self::Error(_))
    }
}

fn write_line(stream: &mut Stream, value: &impl serde::Serialize) -> io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())
}

/// Listen for later launches and requests on background threads, one per
/// connection, so a request that keeps streaming does not hold up others.
fn listen(id: &str) -> io::Result<async_channel::Receiver<Incoming>> {
    let listener = ListenerOptions::new()
        .name(socket_name(id)?)
        // A socket file left by a crash on Unix; named pipes vanish with
        // their process. Only reached when nobody answered on it.
        .try_overwrite(true)
        .create_sync()?;
    let (sender, receiver) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("dowse listener".into())
        .spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let sender = sender.clone();
                std::thread::Builder::new()
                    .name("dowse connection".into())
                    .spawn(move || serve(stream, &sender))
                    .ok();
            }
        })?;
    Ok(receiver)
}

/// Read one message from a connection, hand it to the app and write its
/// answer back.
fn serve(stream: Stream, sender: &async_channel::Sender<Incoming>) {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    if stream.read_line(&mut line).is_err() {
        return;
    }
    let stream = stream.get_mut();
    let message = match serde_json::from_str::<ClientMessage>(&line) {
        Ok(message) => message,
        Err(error) => {
            let error = format!(
                "dowse {} could not read the request ({error}); \
                 is the command line from another version?",
                env!("CARGO_PKG_VERSION")
            );
            write_line(stream, &Frame::Error(error)).ok();
            return;
        }
    };
    match message {
        ClientMessage::Launch(command) => {
            if sender.send_blocking(Incoming::Launch(command)).is_ok() {
                write_line(stream, &Frame::Done).ok();
            }
        }
        ClientMessage::Request(envelope) if envelope.version != PROTOCOL_VERSION => {
            let error = format!(
                "the running dowse {} speaks protocol {PROTOCOL_VERSION}, this command line {}; \
                 quit dowse and try again",
                env!("CARGO_PKG_VERSION"),
                envelope.version
            );
            write_line(stream, &Frame::Error(error)).ok();
        }
        ClientMessage::Request(envelope) => {
            let (reply, answers) = async_channel::unbounded();
            if sender
                .send_blocking(Incoming::Request(envelope, reply))
                .is_err()
            {
                return;
            }
            while let Ok(frame) = answers.recv_blocking() {
                let last = frame.is_last();
                if write_line(stream, &frame).is_err() || last {
                    break;
                }
            }
        }
    }
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

    fn test_socket(name: &str) -> String {
        format!("dowse-test-{name}-{}.sock", std::process::id())
    }

    #[test]
    fn a_second_launch_hands_its_command_to_the_first() {
        let id = test_socket("launch");
        let Launch::Primary(incoming) = claim_named(&id, &Command::Start) else {
            panic!("the first launch should run the app");
        };
        let add = Command::Add(vec![PathBuf::from("/src/api")]);
        assert!(matches!(claim_named(&id, &add), Launch::Forwarded));
        match incoming.recv_blocking().unwrap() {
            Incoming::Launch(command) => assert_eq!(command, add),
            Incoming::Request(..) => panic!("expected a launch"),
        }
    }

    #[test]
    fn requests_get_every_frame_of_their_answer() {
        let id = test_socket("request");
        let Launch::Primary(incoming) = claim_named(&id, &Command::Start) else {
            panic!("the first launch should run the app");
        };
        std::thread::spawn(move || {
            while let Ok(Incoming::Request(envelope, reply)) = incoming.recv_blocking() {
                assert_eq!(envelope.request, Request::Status);
                reply
                    .send_blocking(Frame::Message(envelope.cwd.display().to_string()))
                    .unwrap();
                reply.send_blocking(Frame::Done).unwrap();
            }
        });
        let frames: Vec<Frame> = Connection::open(&id)
            .unwrap()
            .request(Request::Status, Path::new("/work"))
            .unwrap()
            .collect::<io::Result<_>>()
            .unwrap();
        assert_eq!(
            frames,
            [
                Frame::Message(Path::new("/work").display().to_string()),
                Frame::Done
            ]
        );
    }

    #[test]
    fn waits_for_the_running_app_to_go() {
        let id = test_socket("gone");
        assert!(wait_until_named_gone(&id, Duration::ZERO));
        let Launch::Primary(_incoming) = claim_named(&id, &Command::Start) else {
            panic!("the first launch should run the app");
        };
        let started = Instant::now();
        assert!(!wait_until_named_gone(&id, Duration::from_millis(300)));
        assert!(started.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn socket_ids_differ_by_settings_directory() {
        let a = socket_id(Path::new("/config/a"));
        assert_eq!(a, socket_id(Path::new("/CONFIG/A")));
        assert_ne!(a, socket_id(Path::new("/config/b")));
    }
}
