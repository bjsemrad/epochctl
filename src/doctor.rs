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
    match crate::oxide::Client::connect(&socket) {
        Ok(mut client) => match client.providers() {
            Ok(providers) => checks.push(Check::new(
                "epochoxide",
                Level::Ok,
                format!("{} providers at {}", providers.len(), socket.display()),
            )),
            Err(err) => checks.push(Check::new("epochoxide", Level::Warn, err.to_string())),
        },
        Err(err) => checks.push(Check::new("epochoxide", Level::Fail, err.to_string())),
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
