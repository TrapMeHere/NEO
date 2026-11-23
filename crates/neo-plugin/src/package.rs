//! `.neopkg`（zip）检查：路径穿越、清单、SHA-256 签名。
//!
//! 签名是清单去掉 `signature` 行之后与入口字节拼在一起的 SHA-256。
//! 正式分发的 Ed25519 验签可以替换 [`content_signature`] 的调用点；
//! 未签名包只在开发者模式加载，并且会被标成风险。

use std::io::{Cursor, Read, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::PluginError;
use crate::manifest::{PluginManifest, parse_manifest};

/// 通过检查的包。
#[derive(Clone, Debug)]
pub struct OpenedPackage {
    /// 清单。
    pub manifest: PluginManifest,
    /// 原始清单文本。
    pub manifest_text: String,
    /// 入口字节（wat 或 wasm）。
    pub entry: Vec<u8>,
    /// 开发者模式下加载的未签名包。
    pub unsigned_dev: bool,
}

/// 计算清单与入口的签名。
pub fn content_signature(manifest_text: &str, entry: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_manifest(manifest_text).as_bytes());
    hasher.update([0]);
    hasher.update(entry);
    hex(&hasher.finalize())
}

/// 打开内存中的 zip。
pub fn open_package(bytes: &[u8], developer_mode: bool) -> Result<OpenedPackage, PluginError> {
    const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;
    const MAX_ENTRIES: usize = 4096;
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(PluginError::Manifest("package is too large".into()));
    }
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| PluginError::Manifest(err.to_string()))?;
    if archive.len() > MAX_ENTRIES {
        return Err(PluginError::Manifest("too many package entries".into()));
    }
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
        let name = file.name().to_owned();
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') || name.contains('\\') {
            return Err(PluginError::Manifest(format!("path escapes package: {name}")));
        }
        if names.iter().any(|existing| existing == &name) {
            return Err(PluginError::Manifest(format!("duplicate package entry: {name}")));
        }
        names.push(name);
    }
    if !names.iter().any(|name| name == "manifest.toml") {
        return Err(PluginError::Manifest("manifest.toml is missing".into()));
    }
    let manifest_text = read_entry(&mut archive, "manifest.toml")?;
    let manifest_text =
        String::from_utf8(manifest_text).map_err(|err| PluginError::Manifest(err.to_string()))?;
    let manifest = parse_manifest(&manifest_text)?;
    if !manifest.api_compatible() {
        return Err(PluginError::ApiIncompatible {
            plugin: manifest.api,
            host: "1.x".into(),
        });
    }
    if !names.iter().any(|name| name == &manifest.entry) {
        return Err(PluginError::Manifest(format!(
            "entry {} is missing",
            manifest.entry
        )));
    }
    let entry = read_entry(&mut archive, &manifest.entry)?;
    let unsigned_dev = if manifest.signature.is_empty() {
        if !developer_mode {
            return Err(PluginError::SignatureInvalid);
        }
        true
    } else {
        let expect = content_signature(&manifest_text, &entry);
        if !expect.eq_ignore_ascii_case(&manifest.signature) {
            return Err(PluginError::SignatureInvalid);
        }
        false
    };
    Ok(OpenedPackage {
        manifest,
        manifest_text,
        entry,
        unsigned_dev,
    })
}

/// 给测试打包。`signature` 为空时不签名。
pub fn pack_neopkg(manifest_text: &str, entry_name: &str, entry: &[u8]) -> Result<Vec<u8>, PluginError> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("manifest.toml", opts)
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
        zip.write_all(manifest_text.as_bytes())
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
        zip.start_file(entry_name, opts)
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
        zip.write_all(entry)
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
        zip.finish()
            .map_err(|err| PluginError::Manifest(err.to_string()))?;
    }
    Ok(cursor.into_inner())
}

fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, PluginError> {
    const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
    let mut file = archive
        .by_name(name)
        .map_err(|err| PluginError::Manifest(err.to_string()))?;
    if file.size() > MAX_ENTRY_BYTES {
        return Err(PluginError::Manifest(format!("entry {name} is too large")));
    }
    let mut buf = Vec::new();
    file.by_ref()
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|err| PluginError::Manifest(err.to_string()))?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(PluginError::Manifest(format!("entry {name} is too large")));
    }
    Ok(buf)
}

fn canonical_manifest(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("signature"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}
