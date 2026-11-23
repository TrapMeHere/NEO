//! 运行时资源查找。字体、预设等外置文件不编进二进制。
//!
//! 搜索顺序：
//! 1. `NEO_ASSET_DIR`
//! 2. 可执行文件目录及其 `assets/`
//! 3. 开发时从可执行文件目录往上查找仓库根的 `assets`
//! 4. 当前工作目录及其 `assets/`

use std::env;
use std::path::{Path, PathBuf};

/// 可能存放资源的根目录（去重后）。
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
        // Linux 安装布局：/usr/bin/neo-desktop + /usr/share/neo/assets
        push(dir.join("../share/neo"));
        push(dir.join("../share/neo/assets"));
        // macOS bundle：Contents/MacOS/neo-desktop + Contents/Resources/assets
        push(dir.join("../Resources"));
        push(dir.join("../Resources/assets"));
        // out/cargo/debug 上到仓库根。测试二进制在 debug/deps 或 debug/build/.../out，要多走几层。
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

/// 在资源根下查找相对路径（例如 `fonts/NotoSansCJKsc-Regular.otf`）。
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

/// 读取资源文件；找不到返回 `None`。
pub fn read_asset(relative: impl AsRef<Path>) -> Option<Vec<u8>> {
    let path = find_asset(relative)?;
    std::fs::read(path).ok()
}
