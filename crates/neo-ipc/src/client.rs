//! 旁路进程启动与行交换。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// 旁路通信错误。
#[derive(Debug)]
pub enum SidecarError {
    /// 找不到可执行文件。
    Missing(PathBuf),
    /// 启动失败。
    Spawn(std::io::Error),
    /// 读写失败。
    Io(std::io::Error),
    /// 协议或 JSON。
    Protocol(String),
    /// 版本不兼容。
    Version {
        /// 编辑器期望。
        expected: u32,
        /// 旁路报告。
        got: u32,
    },
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "missing sidecar {}", path.display()),
            Self::Spawn(err) => write!(f, "spawn: {err}"),
            Self::Io(err) => write!(f, "io: {err}"),
            Self::Protocol(msg) => write!(f, "protocol: {msg}"),
            Self::Version { expected, got } => {
                write!(f, "version mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for SidecarError {}

/// 解析旁路可执行文件路径：`NEO_<NAME>_EXE`，否则与当前 exe 同目录。
pub fn resolve_sidecar(stem: &str) -> PathBuf {
    let env_key = format!("NEO_{}_EXE", stem.replace('-', "_").to_ascii_uppercase());
    if let Ok(path) = std::env::var(&env_key)
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    let file = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        return dir.join(&file);
    }
    PathBuf::from(file)
}

/// 启动旁路进程（stdin/stdout 管道，stderr 继承）。
pub fn spawn_sidecar(stem: &str) -> Result<Child, SidecarError> {
    let path = resolve_sidecar(stem);
    if !path.exists() {
        return Err(SidecarError::Missing(path));
    }
    Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(SidecarError::Spawn)
}

/// 一行请求、一行应答的旁路（同步用）。
pub struct SidecarProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl SidecarProcess {
    /// 启动。
    pub fn spawn(stem: &str) -> Result<Self, SidecarError> {
        let mut child = spawn_sidecar(stem)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::Protocol("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::Protocol("missing stdout".into()))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// 写一行并读一行。
    pub fn exchange(&mut self, line: &str) -> Result<String, SidecarError> {
        writeln!(self.stdin, "{line}").map_err(SidecarError::Io)?;
        self.stdin.flush().map_err(SidecarError::Io)?;
        let mut reply = String::new();
        self.stdout.read_line(&mut reply).map_err(SidecarError::Io)?;
        while reply.ends_with('\n') || reply.ends_with('\r') {
            reply.pop();
        }
        if reply.is_empty() {
            return Err(SidecarError::Protocol("empty reply".into()));
        }
        Ok(reply)
    }

    /// 可执行路径（诊断用）。
    pub fn path_hint(stem: &str) -> PathBuf {
        resolve_sidecar(stem)
    }
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
