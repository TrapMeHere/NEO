//! NEO launcher: checks sidecar version hints, then starts the editor.
//!
//! The editor still starts when sidecars are missing; the corresponding features degrade in the editor.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let editor = dir.join(if cfg!(windows) {
        "neo-editor.exe"
    } else {
        "neo-editor"
    });

    let mut missing = Vec::new();
    for name in ["neo-sync", "neo-imagery"] {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        if !dir.join(&file).exists() {
            missing.push(file);
        }
    }
    if !missing.is_empty() {
        eprintln!(
            "NEO: optional modules missing (features disabled): {}",
            missing.join(", ")
        );
    }

    if !editor.exists() {
        eprintln!("NEO: missing {}", editor.display());
        std::process::exit(1);
    }

    let status = Command::new(&editor)
        .current_dir(&dir)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("NEO: failed to start editor: {err}");
            std::process::exit(1);
        });
    std::process::exit(status.code().unwrap_or(1));
}
