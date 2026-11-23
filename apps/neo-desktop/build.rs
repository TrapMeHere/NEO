//! Windows 资源：给可执行文件嵌入应用图标与版本信息。
//!
//! 旧依赖 `winres` 有两处问题：
//! 1. 把 `rc.exe` 产出的 `.res` 当成 `.lib` 写盘，链接器无法使用；
//! 2. 本包同时有 lib + bin 时，`rustc-link-lib` 不一定进最终 exe。
//!
//! 这里直接跑 Windows SDK 的 `rc.exe`，再用 `rustc-link-arg-bins`
//! 把 `.res` 交给最终二进制的链接步骤。

fn main() {
    #[cfg(windows)]
    windows::embed();
}

#[cfg(windows)]
mod windows {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn embed() {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
        let icon = manifest_dir
            .join("../../assets/neo.ico")
            .canonicalize()
            .expect("assets/neo.ico must exist");
        println!("cargo:rerun-if-changed={}", icon.display());

        let icon_rc = icon.display().to_string().replace('\\', "\\\\");
        let rc_path = out_dir.join("neo.rc");
        let res_path = out_dir.join("neo.res");
        let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let mut parts = version
            .split('.')
            .map(|part| {
                part.split(['-', '+'])
                    .next()
                    .unwrap_or("0")
                    .parse::<u32>()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();
        parts.resize(4, 0);
        let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
        let rc = format!(
            r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "NEO Project"
      VALUE "FileDescription", "NEO - Next Editor for OSM"
      VALUE "FileVersion", "{version}"
      VALUE "ProductName", "NEO - Next Editor for OSM"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "OSM data (c) OpenStreetMap contributors (ODbL)"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
1 ICON "{icon_rc}"
"#
        );
        fs::write(&rc_path, rc).expect("write neo.rc");

        let rc_exe = find_rc_exe()
            .expect("rc.exe not found. Install Windows 10/11 SDK (Resource Compiler) for MSVC builds.");
        let status = Command::new(&rc_exe)
            .arg(format!("/fo{}", res_path.display()))
            .arg(&rc_path)
            .status()
            .unwrap_or_else(|err| panic!("run rc.exe ({}): {err}", rc_exe.display()));
        if !status.success() {
            panic!("rc.exe failed with {status}");
        }
        if !res_path.is_file() {
            panic!("rc.exe did not write {}", res_path.display());
        }

        // 只链进 bin，避免无效地塞进 rlib。MSVC link.exe 直接接受 .res。
        println!("cargo:rustc-link-arg-bins={}", res_path.display());
    }

    fn find_rc_exe() -> Option<PathBuf> {
        if let Ok(path) = env::var("RC") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(path);
            }
        }
        if let Ok(output) = Command::new("where.exe").arg("rc.exe").output()
            && output.status.success()
            && let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next()
        {
            let path = PathBuf::from(line.trim());
            if path.is_file() {
                return Some(path);
            }
        }
        let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
        if !kits.is_dir() {
            return None;
        }
        let mut versions: Vec<_> = fs::read_dir(kits)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.join(r"x64\rc.exe").is_file())
            .collect();
        versions.sort();
        let latest = versions.pop()?;
        Some(latest.join(r"x64\rc.exe"))
    }
}
