//! 影像旁路消息。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::client::{SidecarError, spawn_sidecar};

/// 影像旁路协议版本。
pub const PROTOCOL_VERSION: u32 = super::PROTOCOL_VERSION;

/// 编辑器 → 影像旁路。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageryRequest {
    /// 握手。
    Hello {
        /// 编辑器侧协议版本。
        version: u32,
    },
    /// 下载一张图（含天地图请求头策略）。
    Fetch {
        /// 完整 URL。
        url: String,
    },
    /// 计算 TMS 请求缩放级别（含天地图 z18 上限）。
    CapZoom {
        /// 预设 id。
        preset_id: String,
        /// URL 模板。
        url: String,
        /// 相机缩放。
        camera_zoom: f64,
        /// 预设声明的上限（没有则 22）。
        preset_max_zoom: Option<u32>,
    },
}

/// 影像旁路 → 编辑器。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageryResponse {
    /// 握手。
    Hello {
        /// 旁路协议版本。
        version: u32,
        /// 是否兼容。
        ok: bool,
    },
    /// 下载成功（base64）。
    Fetched {
        /// 图片字节。
        #[serde(with = "b64")]
        bytes: Vec<u8>,
    },
    /// 缩放级别。
    Zoom {
        /// 请求用的整数级别。
        zoom: u32,
    },
    /// 错误。
    Error {
        /// 消息。
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

/// 带请求 id 的信封（影像并发）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImageryEnvelope<T> {
    /// 协议版本。
    pub v: u32,
    /// 请求/应答配对 id。
    pub id: u64,
    /// 载荷。
    #[serde(flatten)]
    pub body: T,
}

type PendingMap = Arc<Mutex<HashMap<u64, Sender<ImageryResponse>>>>;

/// 影像会话：支持并发 fetch。
pub struct ImagerySession {
    _child: Child,
    stdin: Mutex<ChildStdin>,
    pending: PendingMap,
    next_id: AtomicU64,
    ready: bool,
}

impl ImagerySession {
    /// 启动并握手。
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

    /// 是否可用。
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// 同步调用（握手、CapZoom）。
    pub fn call(&self, request: ImageryRequest) -> Result<ImageryResponse, SidecarError> {
        let (tx, rx) = channel();
        self.send(request, tx)?;
        rx.recv()
            .map_err(|_| SidecarError::Protocol("imagery reply channel closed".into()))
    }

    /// 异步下载：立刻返回接收端。
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
