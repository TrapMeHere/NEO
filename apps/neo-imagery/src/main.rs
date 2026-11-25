//! NEO imagery sidecar: tile downloads and zoom caps.
//!
//! Reads JSON lines carrying an `id` from stdin and writes the matching reply to stdout. Downloads run concurrently on background threads.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use neo_ipc::{ImageryEnvelope, ImageryRequest, ImageryResponse, PROTOCOL_VERSION};

/// Tianditu keys are restricted to browser access. This header set matches a browser opening tiles, without expiring cookies.
const TIANDITU_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36";

fn main() {
    let stdin = BufReader::new(std::io::stdin());
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    for line in stdin.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let parsed = match serde_json::from_str::<ImageryEnvelope<ImageryRequest>>(&line) {
            Ok(parsed) => parsed,
            Err(err) => {
                let _ = write_reply(
                    &stdout,
                    0,
                    ImageryResponse::Error {
                        message: err.to_string(),
                    },
                );
                continue;
            }
        };
        if parsed.v != PROTOCOL_VERSION && !matches!(parsed.body, ImageryRequest::Hello { .. }) {
            let _ = write_reply(
                &stdout,
                parsed.id,
                ImageryResponse::Hello {
                    version: PROTOCOL_VERSION,
                    ok: false,
                },
            );
            continue;
        }
        match parsed.body {
            ImageryRequest::Hello { version } => {
                let _ = write_reply(
                    &stdout,
                    parsed.id,
                    ImageryResponse::Hello {
                        version: PROTOCOL_VERSION,
                        ok: version == PROTOCOL_VERSION,
                    },
                );
            }
            ImageryRequest::CapZoom {
                preset_id,
                url,
                camera_zoom,
                preset_max_zoom,
            } => {
                let zoom = cap_zoom(&preset_id, &url, camera_zoom, preset_max_zoom);
                let _ = write_reply(&stdout, parsed.id, ImageryResponse::Zoom { zoom });
            }
            ImageryRequest::Fetch { url } => {
                let out = stdout.clone();
                let id = parsed.id;
                std::thread::spawn(move || {
                    let response = match fetch_one(&url) {
                        Ok(bytes) => ImageryResponse::Fetched { bytes },
                        Err(message) => ImageryResponse::Error { message },
                    };
                    let _ = write_reply(&out, id, response);
                });
            }
        }
    }
}

fn write_reply(stdout: &Mutex<std::io::Stdout>, id: u64, body: ImageryResponse) -> Result<(), ()> {
    let envelope = ImageryEnvelope {
        v: PROTOCOL_VERSION,
        id,
        body,
    };
    let line = serde_json::to_string(&envelope).map_err(|_| ())?;
    let mut guard = stdout.lock().map_err(|_| ())?;
    writeln!(guard, "{line}").map_err(|_| ())?;
    guard.flush().map_err(|_| ())?;
    Ok(())
}

fn cap_zoom(preset_id: &str, url: &str, camera_zoom: f64, preset_max_zoom: Option<u32>) -> u32 {
    let mut cap = preset_max_zoom.unwrap_or(22);
    if preset_id == "tianditu" || url.contains("tianditu.gov.cn") {
        cap = cap.min(18);
    }
    camera_zoom.floor().clamp(0.0, f64::from(cap)) as u32
}

fn fetch_one(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let mut request = client.get(url);
    if url.contains("tianditu.gov.cn") {
        request = request
            .header("User-Agent", TIANDITU_USER_AGENT)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,zh-TW;q=0.8,en;q=0.7,ja;q=0.6")
            .header("Cache-Control", "max-age=0")
            .header(
                "Sec-CH-UA",
                "\"Chromium\";v=\"158\", \"Not.A/Brand\";v=\"24\", \"Google Chrome\";v=\"158\"",
            )
            .header("Sec-CH-UA-Mobile", "?0")
            .header("Sec-CH-UA-Platform", "\"Windows\"")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "cross-site")
            .header("Sec-Fetch-User", "?1")
            .header("Upgrade-Insecure-Requests", "1");
    } else {
        request = request.header("User-Agent", "NEO/0.1 (imagery)");
    }
    let response = request.send().map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| err.to_string())
}
