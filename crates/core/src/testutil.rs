//! Tiny blocking HTTP server for unit tests. Answers each connection with the next canned response.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) struct MockServer {
    pub base: String,
    hits: Arc<AtomicUsize>,
    pub last_request: Arc<Mutex<String>>,
}

impl MockServer {
    /// Each response is (status, extra header lines ending in \r\n, body).
    /// After the list runs out, the last response repeats.
    pub fn start(responses: Vec<(u16, &'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let last_request = Arc::new(Mutex::new(String::new()));
        let (h, lr) = (hits.clone(), last_request.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                *lr.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
                let i = h.fetch_add(1, Ordering::SeqCst);
                let (status, extra, body) = &responses[i.min(responses.len() - 1)];
                let resp = format!(
                    "HTTP/1.1 {status} X\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        Self {
            base,
            hits,
            last_request,
        }
    }

    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}
