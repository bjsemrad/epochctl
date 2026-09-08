use crate::config::{which, Config};
use anyhow::Result;
use serde_json::Value;
use std::fmt;
use std::process::Command;

/// Why a Quickshell IPC call did not do what was asked.
///
/// The distinction matters because `qs ipc call` exits 0 for most of these: it prints
/// "Target not found." on stdout and reports success, so a keybinding wired straight to `qs`
/// fails silently forever. Everything here is recovered by reading what `qs` printed.
#[derive(Debug)]
pub enum IpcError {
    /// `qs`/`quickshell` is not installed or not on PATH.
    QsMissing(String),
    /// The config directory has no shell.qml, or does not exist.
    ConfigMissing { config_dir: String, detail: String },
    /// The config is valid but no shell process is running it.
    ShellNotRunning { config_dir: String },
    /// The shell is running but exposes no such IPC target.
    TargetNotFound { target: String },
    /// The target exists but has no such function.
    FunctionNotFound { target: String, function: String },
    /// The handler ran and reported a failure of its own.
    Handler { message: String, known: Vec<String> },
    /// `qs` failed in a way none of the above covers.
    Other(String),
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QsMissing(binary) => write!(
                f,
                "cannot find the Quickshell CLI ({binary}).\n\
                 Install quickshell, or point epochctl at it with EPOCHCTL_QS."
            ),
            Self::ConfigMissing { config_dir, detail } => write!(
                f,
                "no Quickshell config at {config_dir} ({detail}).\n\
                 Pass --config <dir>, or set EPOCHSHELL_CONFIG_DIR."
            ),
            Self::ShellNotRunning { config_dir } => write!(
                f,
                "the shell is not running for {config_dir}.\n\
                 Start it with `systemctl --user start epochshell.service`."
            ),
            Self::TargetNotFound { target } => write!(
                f,
                "the running shell exposes no \"{target}\" IPC target.\n\
                 It is probably older than this epochctl -- rebuild and restart epochshell."
            ),
            Self::FunctionNotFound { target, function } => write!(
                f,
                "the \"{target}\" IPC target has no function \"{function}\".\n\
                 The running shell is probably older than this epochctl."
            ),
            Self::Handler { message, known } if !known.is_empty() => {
                write!(f, "{message}\nKnown: {}", known.join(", "))
            }
            Self::Handler { message, .. } => write!(f, "{message}"),
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl IpcError {
    /// Exit code for this failure. Distinct codes let a script tell "shell is down" from
    /// "that feature does not exist in this build" without parsing messages.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::QsMissing(_) => 4,
            Self::ConfigMissing { .. } => 4,
            Self::ShellNotRunning { .. } => 3,
            Self::TargetNotFound { .. } | Self::FunctionNotFound { .. } => 5,
            Self::Handler { .. } => 1,
            Self::Other(_) => 1,
        }
    }
}

/// What an IPC handler answered.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A handler that returned a JSON object, which is what every Epoch target does.
    Json(Value),
    /// A handler that returned nothing. Shells predating the JSON contract declare their
    /// functions `: void`, so an empty response is a success, not a failure.
    Empty,
    /// Anything else `qs` printed, such as `ipc prop get` returning a bare `false`.
    Text(String),
}

impl Reply {
    pub fn as_value(&self) -> Value {
        match self {
            Self::Json(value) => value.clone(),
            Self::Empty => serde_json::json!({ "ok": true }),
            Self::Text(text) => serde_json::json!({ "ok": true, "output": text }),
        }
    }

    pub fn field(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Json(value) => value.get(key),
            _ => None,
        }
    }

    pub fn bool_field(&self, key: &str) -> Option<bool> {
        self.field(key).and_then(Value::as_bool)
    }
}

pub struct QsClient<'a> {
    config: &'a Config,
}

impl<'a> QsClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    fn base_args(&self) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            self.config.config_dir.display().to_string(),
        ];
        if let Some(instance) = &self.config.instance {
            args.push("-i".to_string());
            args.push(instance.clone());
        }
        if let Some(pid) = self.config.pid {
            args.push("--pid".to_string());
            args.push(pid.to_string());
        }
        if self.config.newest {
            args.push("-n".to_string());
        }
        if self.config.any_display {
            args.push("--any-display".to_string());
        }
        args
    }

    /// The exact `qs` invocation a call turns into, for `--dry-run` and `doctor`.
    pub fn command_line(&self, ipc_args: &[String]) -> String {
        let mut parts = vec![self.config.qs_binary.clone()];
        parts.extend(self.base_args());
        parts.push("ipc".to_string());
        parts.extend(ipc_args.iter().cloned());
        parts
            .into_iter()
            .map(|part| {
                if part.contains(' ') {
                    format!("{part:?}")
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn run(&self, ipc_args: &[String]) -> Result<String, IpcError> {
        if which(&self.config.qs_binary).is_none() {
            return Err(IpcError::QsMissing(self.config.qs_binary.clone()));
        }
        let mut command = Command::new(&self.config.qs_binary);
        command.args(self.base_args());
        command.arg("ipc");
        command.args(ipc_args);

        let output = command
            .output()
            .map_err(|err| IpcError::Other(format!("running {}: {err}", self.config.qs_binary)))?;

        // Quickshell reports IPC-level problems on stdout with a zero exit, and process-level
        // problems on stdout with 255, so both streams are searched for the known markers rather
        // than trusting the status.
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let combined = format!("{stdout}\n{stderr}");

        if let Some(err) = self.classify(&combined, ipc_args) {
            return Err(err);
        }
        if !output.status.success() && stdout.is_empty() {
            let detail = if stderr.is_empty() {
                format!("{} exited with {}", self.config.qs_binary, output.status)
            } else {
                stderr
            };
            return Err(IpcError::Other(detail));
        }
        Ok(stdout)
    }

    fn classify(&self, text: &str, ipc_args: &[String]) -> Option<IpcError> {
        classify_output(
            text,
            &self.config.config_dir.display().to_string(),
            ipc_args,
        )
    }

    /// Call `target.function(args...)`.
    pub fn call(&self, target: &str, function: &str, args: &[&str]) -> Result<Reply, IpcError> {
        let ipc_args = self.args_for(target, function, args);
        let raw = self.run(&ipc_args)?;
        parse_reply(raw)
    }

    /// Read an IPC property.
    pub fn prop(&self, target: &str, property: &str) -> Result<String, IpcError> {
        self.run(&[
            "prop".to_string(),
            "get".to_string(),
            target.to_string(),
            property.to_string(),
        ])
    }

    /// Every target the running shell exposes, as printed by `qs ipc show`.
    pub fn targets(&self) -> Result<Vec<String>, IpcError> {
        let raw = self.run(&["show".to_string()])?;
        Ok(raw
            .lines()
            .filter_map(|line| line.strip_prefix("target "))
            .map(|name| name.trim().to_string())
            .collect())
    }

    pub fn args_for(&self, target: &str, function: &str, args: &[&str]) -> Vec<String> {
        let mut ipc_args = vec!["call".to_string(), target.to_string(), function.to_string()];
        ipc_args.extend(args.iter().map(|arg| (*arg).to_string()));
        ipc_args
    }
}

/// Map what `qs` printed onto a typed failure.
///
/// Quickshell reports IPC-level problems on stdout with a *zero* exit status, so this text is the
/// only signal there is: `qs ipc call panel toggle audio` against a shell with no `panel` target
/// prints "Target not found." and exits 0.
fn classify_output(text: &str, config_dir: &str, ipc_args: &[String]) -> Option<IpcError> {
    let config_dir = config_dir.to_string();
    if text.contains("Target not found.") {
        return Some(IpcError::TargetNotFound {
            target: ipc_args.get(1).cloned().unwrap_or_default(),
        });
    }
    if text.contains("Function not found.") {
        return Some(IpcError::FunctionNotFound {
            target: ipc_args.get(1).cloned().unwrap_or_default(),
            function: ipc_args.get(2).cloned().unwrap_or_default(),
        });
    }
    if text.contains("No running instances") {
        return Some(IpcError::ShellNotRunning { config_dir });
    }
    if text.contains("Could not open config file")
        || text.contains("Could not find")
        || text.contains("shell.qml")
    {
        let detail = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no shell.qml")
            .trim()
            .to_string();
        return Some(IpcError::ConfigMissing { config_dir, detail });
    }
    None
}

/// Turn a handler's stdout into a [`Reply`], or the failure it reported.
fn parse_reply(raw: String) -> Result<Reply, IpcError> {
    if raw.is_empty() {
        return Ok(Reply::Empty);
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) if value.is_object() => {
            if value.get("ok").and_then(Value::as_bool) == Some(false) {
                let message = value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("the shell rejected the request")
                    .to_string();
                let known = value
                    .get("known")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                return Err(IpcError::Handler { message, known });
            }
            Ok(Reply::Json(value))
        }
        _ => Ok(Reply::Text(raw)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn target_not_found_is_recovered_from_stdout() {
        let ipc = args(&["call", "panel", "toggle", "audio"]);
        let err = classify_output("Target not found.", "/cfg", &ipc).expect("classified");
        match err {
            IpcError::TargetNotFound { target } => assert_eq!(target, "panel"),
            other => panic!("expected TargetNotFound, got {other:?}"),
        }
        assert_eq!(err_code("Target not found.", &ipc), 5);
    }

    #[test]
    fn function_not_found_names_both_halves() {
        let ipc = args(&["call", "launcher", "nope"]);
        match classify_output("Function not found.", "/cfg", &ipc).expect("classified") {
            IpcError::FunctionNotFound { target, function } => {
                assert_eq!(target, "launcher");
                assert_eq!(function, "nope");
            }
            other => panic!("expected FunctionNotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_stopped_shell_is_distinct_from_a_missing_target() {
        let ipc = args(&["call", "shell", "ping"]);
        let text = "No running instances for \"/cfg/shell.qml\"";
        match classify_output(text, "/cfg", &ipc).expect("classified") {
            IpcError::ShellNotRunning { config_dir } => assert_eq!(config_dir, "/cfg"),
            other => panic!("expected ShellNotRunning, got {other:?}"),
        }
        assert_eq!(err_code(text, &ipc), 3);
    }

    #[test]
    fn a_successful_call_is_not_classified_as_an_error() {
        let ipc = args(&["call", "shell", "ping"]);
        assert!(classify_output("{\"ok\":true,\"pong\":true}", "/cfg", &ipc).is_none());
        assert!(classify_output("", "/cfg", &ipc).is_none());
    }

    #[test]
    fn a_void_handler_answering_nothing_is_a_success() {
        // Shells predating the JSON contract declare their IPC functions `: void`.
        match parse_reply(String::new()).expect("empty reply is ok") {
            Reply::Empty => {}
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn a_handler_failure_carries_its_known_values() {
        let raw = r#"{"ok":false,"error":"unknown panel \"localsend\"","known":["audio","wifi"]}"#;
        match parse_reply(raw.to_string()) {
            Err(IpcError::Handler { message, known }) => {
                assert!(message.contains("localsend"));
                assert_eq!(known, vec!["audio".to_string(), "wifi".to_string()]);
            }
            other => panic!("expected Handler error, got {other:?}"),
        }
    }

    #[test]
    fn a_json_reply_exposes_its_fields() {
        let reply = parse_reply(r#"{"ok":true,"open":true}"#.to_string()).expect("parsed");
        assert_eq!(reply.bool_field("open"), Some(true));
    }

    #[test]
    fn a_bare_property_read_stays_text() {
        match parse_reply("false".to_string()).expect("parsed") {
            Reply::Text(text) => assert_eq!(text, "false"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    fn err_code(text: &str, ipc: &[String]) -> i32 {
        classify_output(text, "/cfg", ipc)
            .expect("classified")
            .exit_code()
    }
}
