//! Imagery sidecar messages.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::client::{SidecarError, spawn_sidecar};

/// Imagery sidecar protocol version.
pub const PROTOCOL_VERSION: u32 = super::PROTOCOL_VERSION;

/// Editor → imagery sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageryRequest {
    /// Handshake.
    Hello {
        /// Editor-side protocol version.
        version: u32,
    },
    /// Downloads one image (including the Tianditu request-header policy).
    Fetch {
        /// Full URL.
        url: String,
    },
    /// Computes the TMS request zoom level (including the Tianditu z18 cap).
    CapZoom {
        /// Preset id.
        preset_id: String,
        /// URL template.
        url: String,
        /// Camera zoom.
        camera_zoom: f64,
        /// Upper bound declared by the preset (22 when absent).
        preset_max_zoom: Option<u32>,
    },
}

/// Imagery sidecar → editor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageryResponse {
    /// Handshake.
    Hello {
        /// Sidecar protocol version.
        version: u32,
        /// Whether compatible.
        ok: bool,
    },
    /// Download succeeded (base64).
    Fetched {
        /// Image bytes.
        #[serde(with = "b64")]
        bytes: Vec<u8>,
    },
    /// Zoom level.
    Zoom {
        /// Integer level used for the request.
        zoom: u32,
    },
    /// Error.
    Error {
        /// Message.
        message: String,
    },
}

mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            bytes,
        ))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// Envelope carrying a request id (imagery concurrency).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImageryEnvelope<T> {
    /// Protocol version.
    pub v: u32,
    /// Request/reply pairing id.
    pub id: u64,
    /// Payload.
    #[serde(flatten)]
    pub body: T,
}

type PendingMap = Arc<Mutex<HashMap<u64, Sender<ImageryResponse>>>>;

/// Imagery session: supports concurrent fetches.
pub struct ImagerySession {
    _child: Child,
    stdin: Mutex<ChildStdin>,
    pending: PendingMap,
    next_id: AtomicU64,
    ready: bool,
}

impl ImagerySession {
    /// Starts and performs the handshake.
    pub fn open() -> Result<Self, SidecarError> {
        let mut child = spawn_sidecar("neo-imagery")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::Protocol("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::Protocol("missing stdout".into()))?;
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let reader_map = pending.clone();
        std::thread::Builder::new()
            .name("neo-imagery-reader".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(parsed) = serde_json::from_str::<ImageryEnvelope<ImageryResponse>>(&line) else {
                        continue;
                    };
                    if let Ok(mut map) = reader_map.lock()
                        && let Some(tx) = map.remove(&parsed.id)
                    {
                        let _ = tx.send(parsed.body);
                    }
                }
            })
            .map_err(SidecarError::Io)?;

        let mut session = Self {
            _child: child,
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            ready: false,
        };
        match session.call(ImageryRequest::Hello {
            version: PROTOCOL_VERSION,
        })? {
            ImageryResponse::Hello { ok: true, .. } => {
                session.ready = true;
                Ok(session)
            }
            ImageryResponse::Hello { version, ok: false } => Err(SidecarError::Version {
                expected: PROTOCOL_VERSION,
                got: version,
            }),
            other => Err(SidecarError::Protocol(format!("unexpected hello: {other:?}"))),
        }
    }

    /// Whether usable.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Synchronous call (handshake, CapZoom).
    pub fn call(&self, request: ImageryRequest) -> Result<ImageryResponse, SidecarError> {
        let (tx, rx) = channel();
        self.send(request, tx)?;
        rx.recv()
            .map_err(|_| SidecarError::Protocol("imagery reply channel closed".into()))
    }

    /// Asynchronous download: returns the receiver immediately.
    pub fn fetch_async(&self, url: String) -> Result<Receiver<ImageryResponse>, SidecarError> {
        let (tx, rx) = channel();
        self.send(ImageryRequest::Fetch { url }, tx)?;
        Ok(rx)
    }

    fn send(&self, request: ImageryRequest, tx: Sender<ImageryResponse>) -> Result<(), SidecarError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut map = self
                .pending
                .lock()
                .map_err(|_| SidecarError::Protocol("pending lock poisoned".into()))?;
            map.insert(id, tx);
        }
        let envelope = ImageryEnvelope {
            v: PROTOCOL_VERSION,
            id,
            body: request,
        };
        let line = serde_json::to_string(&envelope).map_err(|err| SidecarError::Protocol(err.to_string()))?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| SidecarError::Protocol("stdin lock poisoned".into()))?;
        writeln!(stdin, "{line}").map_err(SidecarError::Io)?;
        stdin.flush().map_err(SidecarError::Io)?;
        Ok(())
    }
}

impl Drop for ImagerySession {
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}
