use crate::cli::*;
use crate::config::Config;
use crate::doctor;
use crate::output::{pad, Format};
use crate::oxide::{self, OxideError};
use crate::qs::{IpcError, QsClient, Reply};
use serde_json::{json, Value};
use std::fmt;

/// Anything that can stop a command, carrying the exit code it should produce.
#[derive(Debug)]
pub enum Error {
    Ipc(IpcError),
    Oxide(OxideError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ipc(err) => write!(f, "{err}"),
            Self::Oxide(err) => write!(f, "{err}"),
        }
    }
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Ipc(err) => err.exit_code(),
            Self::Oxide(err) => err.exit_code(),
        }
    }
}

impl From<IpcError> for Error {
    fn from(err: IpcError) -> Self {
        Self::Ipc(err)
    }
}

impl From<OxideError> for Error {
    fn from(err: OxideError) -> Self {
        Self::Oxide(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Context {
    pub config: Config,
    pub format: Format,
    pub dry_run: bool,
}

impl Context {
    fn qs(&self) -> QsClient<'_> {
        QsClient::new(&self.config)
    }

    /// Every IPC target the running shell exposes. Used by `shell targets` and `doctor`.
    pub fn qs_targets(&self) -> std::result::Result<Vec<String>, IpcError> {
        self.qs().targets()
    }

    fn oxide(&self) -> Result<oxide::Client> {
        Ok(oxide::Client::connect(&self.config.socket)?)
    }

    /// Run an IPC call, or print what would run under `--dry-run`.
    fn call(&self, target: &str, function: &str, args: &[&str]) -> Result<Option<Reply>> {
        let client = self.qs();
        if self.dry_run {
            let ipc_args = client.args_for(target, function, args);
            println!("{}", client.command_line(&ipc_args));
            return Ok(None);
        }
        Ok(Some(client.call(target, function, args)?))
    }
}

pub fn dispatch(ctx: &Context, command: Command) -> Result<()> {
    match command {
        Command::Launcher { action } => launcher(ctx, action),
        Command::Panel { action } => panel(ctx, action),
        Command::Shell { action } => shell(ctx, action),
        Command::Ping => shell(ctx, ShellAction::Ping),
        Command::Reload { hard } => shell(ctx, ShellAction::Reload { hard }),
        Command::Capture { action } => capture(ctx, action),
        Command::Search(args) => search(ctx, args),
        Command::Activate(args) => activate(ctx, args),
        Command::Providers => providers(ctx),
        Command::Menu { name } => menu(ctx, &name),
        Command::Doctor => doctor::run(ctx),
        Command::Completions { .. } => Ok(()),
    }
}

/// Report the outcome of an action whose only interesting result is that it worked.
fn report_action(ctx: &Context, reply: Option<Reply>, human: &str) {
    let Some(reply) = reply else { return };
    let value = reply.as_value();
    ctx.format.emit(&value, || {
        if !ctx.format.is_json() {
            println!("{human}");
        }
    });
}

fn launcher(ctx: &Context, action: LauncherAction) -> Result<()> {
    match action {
        LauncherAction::Toggle => {
            let reply = ctx.call("launcher", "toggle", &[])?;
            // A shell predating the JSON contract declares toggle as `: void` and answers with
            // nothing, so the open state is only reported when the shell actually sent it.
            let human = match reply.as_ref().and_then(|r| r.bool_field("open")) {
                Some(true) => "launcher opened".to_string(),
                Some(false) => "launcher closed".to_string(),
                None => "launcher toggled".to_string(),
            };
            report_action(ctx, reply, &human);
        }
        LauncherAction::Open => {
            let reply = ctx.call("launcher", "open", &[])?;
            report_action(ctx, reply, "launcher opened");
        }
        LauncherAction::Close => {
            let reply = ctx.call("launcher", "close", &[])?;
            report_action(ctx, reply, "launcher closed");
        }
        LauncherAction::Provider { name } => {
            let reply = ctx.call("launcher", "openProvider", &[&name])?;
            report_action(ctx, reply, &format!("launcher opened on {name}"));
        }
        LauncherAction::Status => {
            if ctx.dry_run {
                println!(
                    "{}",
                    ctx.qs().command_line(&[
                        "prop".into(),
                        "get".into(),
                        "launcher".into(),
                        "isOpen".into()
                    ])
                );
                return Ok(());
            }
            let raw = ctx.qs().prop("launcher", "isOpen")?;
            let open = raw.trim() == "true";
            ctx.format.emit(&json!({ "ok": true, "open": open }), || {
                println!("{}", if open { "open" } else { "closed" });
            });
        }
    }
    Ok(())
}

fn panel(ctx: &Context, action: PanelAction) -> Result<()> {
    match action {
        PanelAction::Toggle { name } => {
            let reply = ctx.call("panel", "toggle", &[&name])?;
            let human = match reply.as_ref().and_then(|r| r.bool_field("open")) {
                Some(true) => format!("{name} opened"),
                Some(false) => format!("{name} closed"),
                None => format!("{name} toggled"),
            };
            report_action(ctx, reply, &human);
        }
        PanelAction::Open { name } => {
            let reply = ctx.call("panel", "open", &[&name])?;
            report_action(ctx, reply, &format!("{name} opened"));
        }
        PanelAction::Close { name } => {
            let reply = ctx.call("panel", "close", &[&name])?;
            report_action(ctx, reply, &format!("{name} closed"));
        }
        PanelAction::CloseAll => {
            let reply = ctx.call("panel", "closeAll", &[])?;
            report_action(ctx, reply, "all panels closed");
        }
        PanelAction::Status { name } => {
            let reply = ctx.call("panel", "status", &[&name])?;
            let Some(reply) = reply else { return Ok(()) };
            let value = reply.as_value();
            ctx.format.emit(&value, || {
                let open = value.get("open").and_then(Value::as_bool).unwrap_or(false);
                println!("{}", if open { "open" } else { "closed" });
            });
        }
        PanelAction::List => {
            let reply = ctx.call("panel", "list", &[])?;
            let Some(reply) = reply else { return Ok(()) };
            let value = reply.as_value();
            ctx.format.emit(&value, || {
                let empty = Vec::new();
                let panels = value
                    .get("panels")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty);
                if panels.is_empty() {
                    println!("no panels registered");
                    return;
                }
                for entry in panels {
                    let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
                    let open = entry.get("open").and_then(Value::as_bool).unwrap_or(false);
                    println!("{} {}", pad(name, 16), if open { "open" } else { "closed" });
                }
            });
        }
    }
    Ok(())
}

fn shell(ctx: &Context, action: ShellAction) -> Result<()> {
    match action {
        ShellAction::Ping => {
            let reply = ctx.call("shell", "ping", &[])?;
            report_action(ctx, reply, "ok");
        }
        ShellAction::Reload { hard } => {
            let function = if hard { "reloadHard" } else { "reload" };
            let reply = ctx.call("shell", function, &[])?;
            report_action(
                ctx,
                reply,
                if hard {
                    "shell reloaded (hard)"
                } else {
                    "shell reloaded"
                },
            );
        }
        ShellAction::Info => {
            let reply = ctx.call("shell", "info", &[])?;
            let Some(reply) = reply else { return Ok(()) };
            let value = reply.as_value();
            ctx.format.emit(&value, || {
                let show = |label: &str, key: &str| {
                    if let Some(field) = value.get(key) {
                        let rendered = match field {
                            Value::String(text) => text.clone(),
                            Value::Array(items) => items
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", "),
                            other => other.to_string(),
                        };
                        println!("{} {rendered}", pad(label, 18));
                    }
                };
                show("instance", "instance_id");
                show("pid", "pid");
                show("config", "config_dir");
                show("screens", "screens");
                show("panels", "panels");
                show("providers", "providers");
                show("backend", "backend_connected");
                show("launched", "launch_time");
            });
        }
        ShellAction::Targets => {
            if ctx.dry_run {
                println!("{}", ctx.qs().command_line(&["show".to_string()]));
                return Ok(());
            }
            let targets = ctx.qs().targets()?;
            ctx.format
                .emit(&json!({ "ok": true, "targets": targets }), || {
                    for target in &targets {
                        println!("{target}");
                    }
                });
        }
    }
    Ok(())
}

fn capture(ctx: &Context, action: CaptureAction) -> Result<()> {
    match action {
        CaptureAction::Screenshot(args) => screenshot(ctx, args),
        CaptureAction::Status => capture_status(ctx),
    }
}

fn screenshot(ctx: &Context, args: ScreenshotArgs) -> Result<()> {
    // The negative flags are only sent when they were actually passed: leaving them out is what
    // lets the backend apply the user's configured defaults rather than this CLI's idea of them.
    let mut params = json!({
        "mode": args.mode.as_str(),
        "select": args.select,
        "cursor": args.cursor,
        "delay": args.delay,
    });
    let object = params.as_object_mut().expect("params is an object");
    if let Some(output) = &args.output {
        object.insert("output".into(), json!(output));
    }
    if let Some(directory) = &args.dir {
        object.insert("directory".into(), json!(directory.display().to_string()));
    }
    for (flag, key) in [
        (args.no_copy, "copy"),
        (args.no_save, "save"),
        (args.no_notify, "notify"),
    ] {
        if flag {
            object.insert(key.into(), json!(false));
        }
    }

    if ctx.dry_run {
        println!("epochoxide api capture.screenshot --params '{params}'");
        return Ok(());
    }

    let mut client = ctx.oxide()?;
    // Region and window selection wait on the user, and --delay waits on the clock; neither fits
    // under the timeout that keeps a launcher query honest.
    client.set_read_timeout(None)?;
    let shot = client.api("capture.screenshot", params)?;

    ctx.format.emit(&shot, || {
        if shot.get("cancelled").and_then(Value::as_bool) == Some(true) {
            println!("cancelled");
            return;
        }
        let text = |key: &str| shot.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag = |key: &str| shot.get(key).and_then(Value::as_bool).unwrap_or(false);
        let number = |key: &str| shot.get(key).and_then(Value::as_u64).unwrap_or(0);

        let show = |label: &str, value: &str| {
            if !value.is_empty() {
                println!("{} {value}", pad(label, 10));
            }
        };
        show("mode", text("mode"));
        show("window", text("window"));
        show("monitor", text("output"));
        show("region", text("geometry"));
        if flag("saved") {
            show("saved", text("path"));
        } else {
            show("cached", text("path"));
        }
        let (width, height) = (number("width"), number("height"));
        let mut size = String::new();
        if width > 0 && height > 0 {
            size.push_str(&format!("{width}×{height}"));
        }
        if number("bytes") > 0 {
            if !size.is_empty() {
                size.push_str(", ");
            }
            size.push_str(&human_bytes(number("bytes")));
        }
        show("size", &size);
        show(
            "clipboard",
            if flag("copied") {
                "copied"
            } else {
                "left alone"
            },
        );
    });
    Ok(())
}

fn capture_status(ctx: &Context) -> Result<()> {
    if ctx.dry_run {
        println!("epochoxide api capture.status");
        return Ok(());
    }
    let status = ctx.oxide()?.api("capture.status", json!({}))?;
    ctx.format.emit(&status, || {
        let text = |key: &str| status.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag = |key: &str| status.get(key).and_then(Value::as_bool).unwrap_or(false);
        println!("{} {}", pad("directory", 12), text("directory"));
        println!("{} {}", pad("filename", 12), text("filename"));
        let defaults: Vec<&str> = [("copy", "copy"), ("save", "save"), ("notify", "notify")]
            .into_iter()
            .filter(|(key, _)| flag(key))
            .map(|(_, name)| name)
            .collect();
        println!(
            "{} {}",
            pad("defaults", 12),
            if defaults.is_empty() {
                "none".to_string()
            } else {
                defaults.join(", ")
            }
        );
        let compositor = text("compositor");
        println!(
            "{} {}",
            pad("compositor", 12),
            if compositor.is_empty() {
                "none responding".to_string()
            } else if flag("window_capture") {
                format!("{compositor}, window capture available")
            } else {
                format!("{compositor}, no window geometry -- use region")
            }
        );
        let empty = Vec::new();
        for tool in status
            .get("tools")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            let path = tool.get("path").and_then(Value::as_str);
            let required = tool
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let purpose = tool.get("purpose").and_then(Value::as_str).unwrap_or("");
            let detail = match path {
                Some(path) => path.to_string(),
                None if required => format!("missing -- required for {purpose}"),
                None => format!("missing -- no {purpose}"),
            };
            println!("{} {detail}", pad(name, 12));
        }
    });
    Ok(())
}

/// A file size in the units a person reads, not bytes.
fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        0..KB => format!("{bytes} B"),
        KB..MB => format!("{:.0} KB", bytes as f64 / KB as f64),
        _ => format!("{:.1} MB", bytes as f64 / MB as f64),
    }
}

fn search(ctx: &Context, args: SearchArgs) -> Result<()> {
    let mut client = ctx.oxide()?;
    // An empty provider list means "every provider that answers queries", which has to be asked
    // for by name: EpochOxide treats an empty list as a request for its own defaults.
    let providers = if args.provider.is_empty() {
        client
            .providers()?
            .into_iter()
            .filter(|provider| provider.supports_query)
            .map(|provider| provider.name)
            .collect()
    } else {
        args.provider.clone()
    };

    let items = client.query(&providers, &args.query, args.limit, args.exact)?;
    let payload = json!({
        "ok": true,
        "query": args.query,
        "providers": providers,
        "items": items.iter().map(|item| json!({
            "provider": item.provider,
            "identifier": item.identifier,
            "text": item.text,
            "subtext": item.subtext,
            "score": item.score,
            "actions": item.actions,
        })).collect::<Vec<_>>(),
    });

    ctx.format.emit(&payload, || {
        if items.is_empty() {
            println!("no results");
            return;
        }
        for item in &items {
            let subtext = if item.subtext.is_empty() {
                String::new()
            } else {
                format!("  {}", item.subtext)
            };
            println!("{} {}{subtext}", pad(&item.provider, 11), item.text);
        }
    });
    Ok(())
}

fn activate(ctx: &Context, args: ActivateArgs) -> Result<()> {
    let mut client = ctx.oxide()?;
    let data = client.activate(
        &args.provider,
        &args.identifier,
        &args.action,
        &args.arguments,
    )?;
    let payload = json!({ "ok": true, "data": data });
    ctx.format.emit(&payload, || println!("ok"));
    Ok(())
}

fn providers(ctx: &Context) -> Result<()> {
    let mut client = ctx.oxide()?;
    let providers = client.providers()?;
    let payload = json!({
        "ok": true,
        "providers": providers.iter().map(|provider| json!({
            "name": provider.name,
            "name_pretty": provider.name_pretty,
            "description": provider.description,
            "prefixes": provider.prefixes,
            "supports_query": provider.supports_query,
        })).collect::<Vec<_>>(),
    });
    ctx.format.emit(&payload, || {
        for provider in &providers {
            println!(
                "{} {} {}",
                pad(&provider.name, 14),
                pad(&provider.prefixes.join(" "), 4),
                provider.description
            );
        }
    });
    Ok(())
}

fn menu(ctx: &Context, name: &str) -> Result<()> {
    let mut client = ctx.oxide()?;
    let items = client.menu(name)?;
    let payload = json!({
        "ok": true,
        "menu": name,
        "items": items.iter().map(|item| json!({
            "identifier": item.identifier,
            "text": item.text,
            "subtext": item.subtext,
        })).collect::<Vec<_>>(),
    });
    ctx.format.emit(&payload, || {
        if items.is_empty() {
            println!("no entries");
            return;
        }
        for item in &items {
            println!("{} {}", pad(&item.text, 34), item.subtext);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_reported_in_units_people_read() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1 KB");
        assert_eq!(human_bytes(305_481), "298 KB");
        assert_eq!(human_bytes(3_500_000), "3.3 MB");
    }
}
