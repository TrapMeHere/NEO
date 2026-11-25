//! Runtime asset lookup. Fonts, presets and other external files are not compiled into the binary.
//!
//! Search order:
//! 1. `NEO_ASSET_DIR`
//! 2. the executable directory and its `assets/`
//! 3. during development, walk up from the executable directory to the repository root's `assets`
//! 4. the current working directory and its `assets/`

use std::env;
use std::path::{Path, PathBuf};

/// Directories that may contain assets (after deduplication).
pub fn asset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push = |path: PathBuf| {
        if path.as_os_str().is_empty() {
            return;
        }
        if !roots.iter().any(|existing| existing == &path) {
            roots.push(path);
        }
    };

    if let Ok(dir) = env::var("NEO_ASSET_DIR") {
        push(PathBuf::from(dir));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        push(dir.to_path_buf());
        push(dir.join("assets"));
        // Linux install layout: /usr/bin/neo-desktop + /usr/share/neo/assets
        push(dir.join("../share/neo"));
        push(dir.join("../share/neo/assets"));
        // macOS bundle: Contents/MacOS/neo-desktop + Contents/Resources/assets
        push(dir.join("../Resources"));
        push(dir.join("../Resources/assets"));
        // out/cargo/debug up to the repository root. Test binaries live in debug/deps or debug/build/.../out, so walk up several levels.
        let mut ancestor = dir.to_path_buf();
        for _ in 0..8 {
            let Some(parent) = ancestor.parent() else {
                break;
            };
            ancestor = parent.to_path_buf();
            push(ancestor.clone());
            push(ancestor.join("assets"));
        }
    }
    if let Ok(cwd) = env::current_dir() {
        push(cwd.clone());
        push(cwd.join("assets"));
    }
    roots
}

/// Look up a relative path under the asset roots (e.g. `fonts/NotoSansCJKsc-Regular.otf`).
pub fn find_asset(relative: impl AsRef<Path>) -> Option<PathBuf> {
    let relative = relative.as_ref();
    for root in asset_roots() {
        let candidate = root.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Read an asset file; returns `None` when not found.
pub fn read_asset(relative: impl AsRef<Path>) -> Option<Vec<u8>> {
    let path = find_asset(relative)?;
    std::fs::read(path).ok()
}
