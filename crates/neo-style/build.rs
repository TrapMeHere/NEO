//! Generate the external preset files `assets/presets/schema.neo` and `suggestions.neo` (not compiled into the binary).
//!
//! If the files already exist, reuse them when Node is absent; otherwise error out telling the user to run `npm ci` first.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("neo-style lives in crates/")
        .to_path_buf();
    let script = root.join("scripts").join("build-presets.mjs");
    let out_dir = root.join("assets").join("presets");
    let schema = out_dir.join("schema.neo");
    let suggestions = out_dir.join("suggestions.neo");

    println!("cargo:rerun-if-changed={}", root.join("package.json").display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("package-lock.json").display()
    );
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=NODE");
    // Do not declare the output file as an input: the script rewrites it every run, otherwise cargo would rerun
    // the build script forever (incremental builds degrade to running node + writing 18 MB every time).
    let _ = std::fs::create_dir_all(&out_dir);
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_owned());

    let status = Command::new(&node)
        .arg(&script)
        .current_dir(&root)
        .env("NEO_PRESET_DIR", &out_dir)
        .status();

    let existing_is_usable = || -> bool {
        let ok = |path: &std::path::Path| {
            std::fs::read(path)
                .map(|bytes| bytes.len() > 64 && bytes.starts_with(b"NEOP"))
                .unwrap_or(false)
        };
        ok(&schema) && ok(&suggestions)
    };
    match status {
        Ok(code) if code.success() => {}
        Ok(code) => {
            if existing_is_usable() {
                println!(
                    "cargo:warning=scripts/build-presets.mjs exited {code}; using existing {}",
                    out_dir.display()
                );
            } else {
                panic!(
                    "scripts/build-presets.mjs failed ({code}) and {} is missing/corrupt. Run `npm ci` then retry.",
                    out_dir.display()
                );
            }
        }
        Err(err) => {
            if existing_is_usable() {
                println!(
                    "cargo:warning=node unavailable ({err}); using existing {}",
                    out_dir.display()
                );
            } else {
                panic!(
                    "cannot run node ({err}) and {} is missing/corrupt. Install Node.js 22, run `npm ci`, then `node scripts/build-presets.mjs`.",
                    out_dir.display()
                );
            }
        }
    }
}
