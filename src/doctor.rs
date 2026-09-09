use crate::config::{runtime_dir, which};
use crate::output::pad;
use crate::run::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok  ",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

struct Check {
    name: &'static str,
    level: Level,
    detail: String,
}

impl Check {
    fn new(name: &'static str, level: Level, detail: impl Into<String>) -> Self {
        Self {
            name,
            level,
            detail: detail.into(),
        }
    }
}

/// Quickshell instances registered for this user, and whether their process is still alive.
///
/// Registration directories outlive the process that made them, so a session that has restarted
/// the shell a few times accumulates stale entries. They are harmless but they make "oldest
/// instance wins" surprising, which is worth surfacing.
fn instances() -> (Vec<u32>, Vec<u32>) {
    let by_pid = runtime_dir().join("quickshell").join("by-pid");
    let mut alive = Vec::new();
    let mut stale = Vec::new();
    let Ok(entries) = fs::read_dir(&by_pid) else {
        return (alive, stale);
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if PathBuf::from(format!("/proc/{pid}")).exists() {
            alive.push(pid);
        } else {
            stale.push(pid);
        }
    }
    alive.sort_unstable();
    stale.sort_unstable();
    (alive, stale)
}

/// Targets epochctl needs, and what stops working without each one.
const REQUIRED_TARGETS: &[(&str, &str)] = &[
    ("launcher", "epochctl launcher"),
    ("panel", "epochctl panel"),
    ("shell", "epochctl shell / ping / reload"),
];

pub fn run(ctx: &Context) -> Result<()> {
    let mut checks = Vec::new();

    match which(&ctx.config.qs_binary) {
        Some(path) => checks.push(Check::new(
            "quickshell cli",
            Level::Ok,
            path.display().to_string(),
        )),
        None => checks.push(Check::new(
            "quickshell cli",
            Level::Fail,
            format!("{} not found on PATH", ctx.config.qs_binary),
        )),
    }

    let config_dir = ctx.config.config_dir.clone();
    let shell_qml = config_dir.join("shell.qml");
    if shell_qml.is_file() {
        checks.push(Check::new(
            "shell config",
            Level::Ok,
            config_dir.display().to_string(),
        ));
    } else {
        checks.push(Check::new(
            "shell config",
            Level::Fail,
            format!("no shell.qml under {}", config_dir.display()),
        ));
    }

    let (alive, stale) = instances();
    let instance_detail = format!(
        "{} running, {} stale registration{}",
        alive.len(),
        stale.len(),
        if stale.len() == 1 { "" } else { "s" }
    );
    checks.push(Check::new(
        "shell instances",
        if alive.is_empty() {
            Level::Fail
        } else if stale.is_empty() {
            Level::Ok
        } else {
            Level::Warn
        },
        instance_detail,
    ));

    // The set of targets is what decides which epochctl commands can work at all, so it is the
    // check most worth reading when a command reports "no such IPC target".
    let mut targets = Vec::new();
    match ctx.qs_targets() {
        Ok(found) => {
            targets = found;
            checks.push(Check::new(
                "ipc targets",
                Level::Ok,
                if targets.is_empty() {
                    "none".to_string()
                } else {
                    targets.join(", ")
                },
            ));
        }
        Err(err) => checks.push(Check::new("ipc targets", Level::Fail, err.to_string())),
    }

    for (target, enables) in REQUIRED_TARGETS {
        let present = targets.iter().any(|found| found == target);
        checks.push(Check::new(
            match *target {
                "launcher" => "target launcher",
                "panel" => "target panel",
                _ => "target shell",
            },
            if present { Level::Ok } else { Level::Warn },
            if present {
                format!("{enables} available")
            } else {
                format!("missing, {enables} will not work")
            },
        ));
    }

    let socket = ctx.config.socket.clone();
    let mut backend_providers: Option<Vec<String>> = None;
    match crate::oxide::Client::connect(&socket) {
        Ok(mut client) => {
            match client.providers() {
                Ok(providers) => {
                    checks.push(Check::new(
                        "epochoxide",
                        Level::Ok,
                        format!("{} providers at {}", providers.len(), socket.display()),
                    ));
                    backend_providers = Some(
                        providers
                            .into_iter()
                            .map(|provider| provider.name)
                            .collect(),
                    );
                }
                Err(err) => checks.push(Check::new("epochoxide", Level::Warn, err.to_string())),
            }
            checks.push(capture(&mut client));
        }
        Err(err) => checks.push(Check::new("epochoxide", Level::Fail, err.to_string())),
    }

    // The shell keeps its own copy of the provider list, refreshed when it reconnects. If that has
    // drifted from what the backend reports, the launcher is showing a stale set -- usually
    // because EpochOxide restarted after the shell did, or the two are on different sockets.
    if let (Some(backend), true) = (
        backend_providers.as_ref(),
        targets.iter().any(|target| target == "shell"),
    ) {
        match shell_providers(ctx) {
            // A shell that does not report providers is simply older than the check.
            Ok(None) => {}
            Ok(Some(shell)) => {
                let missing: Vec<&String> = backend
                    .iter()
                    .filter(|name| !shell.contains(name))
                    .collect();
                let extra: Vec<&String> = shell
                    .iter()
                    .filter(|name| !backend.contains(name))
                    .collect();
                if missing.is_empty() && extra.is_empty() {
                    checks.push(Check::new(
                        "provider sync",
                        Level::Ok,
                        format!("shell and backend agree on {} providers", backend.len()),
                    ));
                } else {
                    let mut detail = format!(
                        "shell sees {} providers, backend has {}",
                        shell.len(),
                        backend.len()
                    );
                    if !missing.is_empty() {
                        detail.push_str(&format!(
                            "; shell is missing {}",
                            missing
                                .iter()
                                .map(|name| name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if !extra.is_empty() {
                        detail.push_str(&format!(
                            "; shell still lists {}",
                            extra
                                .iter()
                                .map(|name| name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    detail.push_str(" (try `epochctl reload`)");
                    checks.push(Check::new("provider sync", Level::Warn, detail));
                }
            }
            Err(err) => checks.push(Check::new("provider sync", Level::Warn, err.to_string())),
        }
    }

    let worst = checks
        .iter()
        .map(|check| check.level)
        .max_by_key(|level| match level {
            Level::Ok => 0,
            Level::Warn => 1,
            Level::Fail => 2,
        })
        .unwrap_or(Level::Ok);

    let payload = json!({
        "ok": worst != Level::Fail,
        "status": worst.as_str(),
        "checks": checks.iter().map(|check| json!({
            "name": check.name,
            "status": check.level.as_str(),
            "detail": check.detail,
        })).collect::<Vec<Value>>(),
        "stale_instances": stale,
        "running_instances": alive,
    });

    ctx.format.emit(&payload, || {
        for check in &checks {
            println!(
                "{} {} {}",
                check.level.marker(),
                pad(check.name, 17),
                check.detail
            );
        }
        if !stale.is_empty() {
            println!();
            println!(
                "Stale instance registrations under {}/quickshell/by-id are left behind by\n\
                 previous shell processes. They are harmless, but they are why `qs` picking the\n\
                 \"oldest\" instance can be surprising; epochctl -n selects the newest.",
                runtime_dir().display()
            );
        }
    });
    Ok(())
}

/// Whether `epochctl capture` can actually take a screenshot here.
///
/// The tools it needs are installed separately from EpochShell, so a missing one is a
/// configuration problem rather than a bug -- and it is only ever noticed at the moment someone
/// presses their screenshot key, which is the worst time to find out.
fn capture(client: &mut crate::oxide::Client) -> Check {
    let status = match client.api("capture.status", json!({})) {
        Ok(status) => status,
        // A backend older than the capture group has no such method; that is version skew, not a
        // broken machine.
        Err(err) => return Check::new("capture", Level::Warn, err.to_string()),
    };
    let empty = Vec::new();
    let tools = status
        .get("tools")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let missing: Vec<&str> = tools
        .iter()
        .filter(|tool| tool.get("path").map(Value::is_null).unwrap_or(true))
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    let required_missing = tools.iter().any(|tool| {
        tool.get("path").map(Value::is_null).unwrap_or(true)
            && tool.get("required").and_then(Value::as_bool) == Some(true)
    });
    let directory = status
        .get("directory")
        .and_then(Value::as_str)
        .unwrap_or("");
    if missing.is_empty() {
        return Check::new("capture", Level::Ok, format!("saving to {directory}"));
    }
    Check::new(
        "capture",
        if required_missing {
            Level::Fail
        } else {
            Level::Warn
        },
        format!("not installed: {}", missing.join(", ")),
    )
}

/// The provider names the running shell currently has, as reported by its `shell` IPC target.
///
/// `Ok(None)` means the shell answered but reports no `providers` field at all, which is a shell
/// older than that addition rather than a shell that has lost its providers. Treating the two the
/// same would warn about drift on every pre-existing install.
fn shell_providers(ctx: &Context) -> std::result::Result<Option<Vec<String>>, crate::qs::IpcError> {
    let reply = crate::qs::QsClient::new(&ctx.config).call("shell", "info", &[])?;
    let Some(items) = reply.field("providers").and_then(Value::as_array) else {
        return Ok(None);
    };
    Ok(Some(
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    ))
}
