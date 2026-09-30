//! A tiny HTTP server for tests, answering each path with a canned reply
//! and recording the requests it saw.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// A request as the server saw it; header names in lowercase.
#[derive(Clone, Debug)]
pub struct Seen {
    pub path: String,
    pub headers: HashMap<String, String>,
}

pub struct TestServer {
    /// `http://127.0.0.1:<port>`.
    pub base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl TestServer {
    /// Serve `routes` (path to reply) until the test ends; other paths get 404.
    pub fn start(routes: Vec<(String, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: HashMap<String, Reply> = routes.into_iter().collect();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = HashMap::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                log.lock().unwrap().push(Seen {
                    path: path.clone(),
                    headers,
                });
                let reply = routes
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| Reply::status(404));
                let mut head = format!(
                    "HTTP/1.1 {} Canned\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in &reply.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).ok();
                stream.write_all(&reply.body).ok();
            }
        });
        Self { base, seen }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}
