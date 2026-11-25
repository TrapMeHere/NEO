//! Windows resources: embed the application icon and version info into the executable.
//!
//! The old `winres` dependency had two problems:
//! 1. It wrote the `.res` produced by `rc.exe` to disk as a `.lib`, which the linker cannot use;
//! 2. When this package has both a lib and a bin, `rustc-link-lib` does not necessarily reach the final exe.
//!
//! Here we run the Windows SDK's `rc.exe` directly, then use `rustc-link-arg-bins`
//! to hand the `.res` to the final binary's link step.

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

        // Link only into the bin, avoiding uselessly stuffing it into the rlib. MSVC link.exe accepts .res directly.
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
