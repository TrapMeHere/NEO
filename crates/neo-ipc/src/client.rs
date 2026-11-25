//! Sidecar process startup and line exchange.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Sidecar communication error.
#[derive(Debug)]
pub enum SidecarError {
    /// Executable not found.
    Missing(PathBuf),
    /// Startup failed.
    Spawn(std::io::Error),
    /// Read/write failed.
    Io(std::io::Error),
    /// Protocol or JSON.
    Protocol(String),
    /// Incompatible version.
    Version {
        /// Expected by the editor.
        expected: u32,
        /// Reported by the sidecar.
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

/// Resolves the sidecar executable path: `NEO_<NAME>_EXE`, otherwise the directory of the current exe.
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

/// Starts a sidecar process (stdin/stdout piped, stderr inherited).
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

/// One-request, one-reply sidecar (for sync).
pub struct SidecarProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl SidecarProcess {
    /// Start.
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

    /// Writes one line and reads one line.
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

    /// Executable path (for diagnostics).
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
