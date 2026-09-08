use anyhow::{bail, Result};
use std::path::PathBuf;

/// Where epochctl finds the two things it talks to: the Quickshell config directory that
/// identifies the shell instance, and the EpochOxide socket.
#[derive(Debug, Clone)]
pub struct Config {
    /// Quickshell config directory, passed to `qs -p`.
    pub config_dir: PathBuf,
    /// The `qs` (or `quickshell`) binary.
    pub qs_binary: String,
    /// EpochOxide's Unix socket.
    pub socket: PathBuf,
    /// Instance selection, forwarded to `qs` when set.
    pub instance: Option<String>,
    pub pid: Option<u32>,
    /// Prefer the newest matching instance instead of the oldest.
    pub newest: bool,
    /// Do not filter instances by the display they were launched on.
    pub any_display: bool,
}

pub const DEFAULT_CONFIG_SUBDIR: &str = "epochshell";
pub const SOCKET_NAME: &str = "epochoxide.sock";

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `$XDG_RUNTIME_DIR` with the same `/tmp` fallback LauncherService.qml uses, so the CLI and the
/// shell agree on the socket even when the runtime dir is unset.
pub fn runtime_dir() -> PathBuf {
    env_path("XDG_RUNTIME_DIR").unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn default_config_dir() -> PathBuf {
    if let Some(dir) = env_path("EPOCHSHELL_CONFIG_DIR") {
        return dir;
    }
    let base = env_path("XDG_CONFIG_HOME")
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(DEFAULT_CONFIG_SUBDIR)
}

pub fn default_socket() -> PathBuf {
    if let Some(socket) = env_path("EPOCHOXIDE_SOCKET") {
        return socket;
    }
    runtime_dir().join(SOCKET_NAME)
}

/// `qs` and `quickshell` are the same binary from the same package; `qs` is the short name and is
/// what the Quickshell docs use, so it is tried first.
fn default_qs_binary() -> String {
    if let Some(binary) = std::env::var_os("EPOCHCTL_QS") {
        if !binary.is_empty() {
            return binary.to_string_lossy().into_owned();
        }
    }
    for candidate in ["qs", "quickshell"] {
        if which(candidate).is_some() {
            return candidate.to_string();
        }
    }
    "qs".to_string()
}

pub fn which(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let path = PathBuf::from(binary);
        return path.is_file().then_some(path);
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

impl Config {
    pub fn resolve(
        config_dir: Option<PathBuf>,
        socket: Option<PathBuf>,
        instance: Option<String>,
        pid: Option<u32>,
        newest: bool,
        any_display: bool,
    ) -> Result<Self> {
        let config_dir = config_dir.unwrap_or_else(default_config_dir);
        if instance.is_some() && pid.is_some() {
            bail!("--instance and --pid select the same thing; pass only one");
        }
        Ok(Self {
            config_dir,
            qs_binary: default_qs_binary(),
            socket: socket.unwrap_or_else(default_socket),
            instance,
            pid,
            newest,
            any_display,
        })
    }
}
