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
        Command::System { action } => system(ctx, action),
        Command::Toggle { action } => toggle(ctx, action),
        Command::Power { action } => power(ctx, action),
        Command::Nix { action } => nix(ctx, action),
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
        CaptureAction::Ocr(args) => ocr(ctx, args),
        CaptureAction::Record(args) => record(ctx, args),
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

fn ocr(ctx: &Context, args: OcrArgs) -> Result<()> {
    let mut params = json!({
        "mode": args.mode.as_str(),
        "select": args.select,
        "delay": args.delay,
        "save": args.keep,
    });
    let object = params.as_object_mut().expect("params is an object");
    if let Some(language) = &args.lang {
        object.insert("language".into(), json!(language));
    }
    if let Some(output) = &args.output {
        object.insert("output".into(), json!(output));
    }
    if let Some(directory) = &args.dir {
        object.insert("directory".into(), json!(directory.display().to_string()));
    }
    if args.no_copy {
        object.insert("copy".into(), json!(false));
    }
    if args.no_notify {
        object.insert("notify".into(), json!(false));
    }

    if ctx.dry_run {
        println!("epochoxide api capture.ocr --params '{params}'");
        return Ok(());
    }

    let mut client = ctx.oxide()?;
    client.set_read_timeout(None)?;
    let result = client.api("capture.ocr", params)?;

    ctx.format.emit(&result, || {
        if result.get("cancelled").and_then(Value::as_bool) == Some(true) {
            println!("cancelled");
            return;
        }
        // The text is the whole point, so it is all that goes to stdout: `epochctl capture ocr >
        // notes.txt` should hold text and nothing else. Whether it was copied is what the
        // notification is for.
        let text = result.get("text").and_then(Value::as_str).unwrap_or("");
        if text.is_empty() {
            println!("no text found");
            return;
        }
        println!("{text}");
    });
    Ok(())
}

fn record(ctx: &Context, args: RecordArgs) -> Result<()> {
    let (method, params) = match args.target {
        RecordTarget::Status => ("capture.recording", json!({})),
        RecordTarget::Stop => {
            let mut params = json!({});
            if args.no_notify {
                params["notify"] = json!(false);
            }
            ("capture.stopRecording", params)
        }
        mode => {
            let mut params = json!({
                "mode": record_mode(mode),
                "select": args.select,
                "delay": args.delay,
            });
            let object = params.as_object_mut().expect("params is an object");
            if let Some(output) = &args.output {
                object.insert("output".into(), json!(output));
            }
            if let Some(directory) = &args.dir {
                object.insert("directory".into(), json!(directory.display().to_string()));
            }
            ("capture.record", params)
        }
    };

    if ctx.dry_run {
        println!("epochoxide api {method} --params '{params}'");
        return Ok(());
    }

    let mut client = ctx.oxide()?;
    // Starting a region recording waits on the user drawing a box, and stopping waits on the
    // recorder finishing the file.
    client.set_read_timeout(None)?;
    let session = client.api(method, params)?;

    ctx.format.emit(&session, || {
        let text = |key: &str| session.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag = |key: &str| session.get(key).and_then(Value::as_bool).unwrap_or(false);
        let number = |key: &str| session.get(key).and_then(Value::as_u64).unwrap_or(0);
        let show = |label: &str, value: &str| {
            if !value.is_empty() {
                println!("{} {value}", pad(label, 9));
            }
        };

        if flag("cancelled") {
            println!("cancelled");
            return;
        }
        if flag("recording") {
            // Starting and asking both answer with a running session; the elapsed time is what
            // tells them apart to a reader.
            show("mode", text("mode"));
            show("monitor", text("output"));
            show("region", text("geometry"));
            show("file", text("path"));
            if number("seconds") > 0 {
                show("running", &human_duration(number("seconds")));
            }
            return;
        }
        // Not recording: either a stop that finished a file, or nothing was going on.
        if text("path").is_empty() {
            println!("not recording");
            return;
        }
        show("saved", text("path"));
        show("length", &human_duration(number("seconds")));
        show("size", &human_bytes(number("bytes")));
    });
    Ok(())
}

/// The API's spelling of a recording mode. `stop` and `status` never reach this.
fn record_mode(target: RecordTarget) -> &'static str {
    match target {
        RecordTarget::Window => "window",
        RecordTarget::Fullscreen => "fullscreen",
        RecordTarget::All => "all",
        _ => "region",
    }
}

/// A duration as a person reads it back: `0:42`, `3:07`, `1:02:13`.
fn human_duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
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
        println!(
            "{} {}",
            pad("recording", 12),
            if flag("record") {
                format!("available, saving to {}", text("recording_directory"))
            } else {
                "wf-recorder is not installed".to_string()
            }
        );
        println!(
            "{} {}",
            pad("ocr", 12),
            if flag("ocr") {
                format!("available, reading {}", text("ocr_language"))
            } else {
                "tesseract is not installed".to_string()
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

fn system(ctx: &Context, action: SystemAction) -> Result<()> {
    let (method, params) = match action {
        SystemAction::Info => ("system.hardware", json!({})),
        SystemAction::Firmware { refresh } => ("system.firmware", json!({ "refresh": refresh })),
    };
    if ctx.dry_run {
        println!("epochoxide api {method} --params '{params}'");
        return Ok(());
    }
    let mut client = ctx.oxide()?;
    // Asking fwupd again talks to its daemon and can take a few seconds.
    if matches!(action, SystemAction::Firmware { refresh: true }) {
        client.set_read_timeout(None)?;
    }
    let data = client.api(method, params)?;

    ctx.format.emit(&data, || match action {
        SystemAction::Info => print_hardware(&data),
        SystemAction::Firmware { .. } => print_firmware(&data),
    });
    Ok(())
}

fn print_hardware(data: &Value) {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let model = [text(data, "vendor"), text(data, "product")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    println!("{} {model}", pad("machine", 11));
    let family = text(data, "family");
    if !family.is_empty() {
        println!("{} {family}", pad("family", 11));
    }
    let bios = text(data, "bios_version");
    if !bios.is_empty() {
        println!("{} {bios}", pad("bios", 11));
    }

    let Some(battery) = data.get("battery").filter(|value| !value.is_null()) else {
        println!("{} none", pad("battery", 11));
        return;
    };
    let number = |key: &str| battery.get(key).and_then(Value::as_u64);
    let name = [
        text(battery, "name"),
        text(battery, "manufacturer"),
        text(battery, "model"),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    println!("{} {name}", pad("battery", 11));
    // Health is the number nobody's desktop shows and everybody wants.
    if let Some(health) = number("health") {
        let wear = match (number("full"), number("design_full")) {
            (Some(full), Some(design)) => {
                format!("  ({full}/{design} {})", text(battery, "unit"))
            }
            _ => String::new(),
        };
        println!("{} {health}%{wear}", pad("health", 11));
    }
    if let Some(cycles) = number("cycle_count") {
        println!("{} {cycles}", pad("cycles", 11));
    }
    if let Some(capacity) = number("capacity") {
        println!(
            "{} {capacity}%  {}",
            pad("charge", 11),
            text(battery, "status").to_lowercase()
        );
    }
    match (
        number("charge_start_threshold"),
        number("charge_end_threshold"),
    ) {
        (Some(start), Some(end)) => println!("{} {start}-{end}%", pad("limits", 11)),
        (None, Some(end)) => println!("{} up to {end}%", pad("limits", 11)),
        _ => {}
    }
}

fn print_firmware(data: &Value) {
    if data.get("available").and_then(Value::as_bool) != Some(true) {
        println!(
            "{}",
            data.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("firmware updates are unavailable")
        );
        return;
    }
    let empty = Vec::new();
    let updates = data
        .get("updates")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if updates.is_empty() {
        println!("firmware is up to date");
        return;
    }
    for update in updates {
        let field = |key: &str| update.get(key).and_then(Value::as_str).unwrap_or("");
        println!(
            "{} {} -> {}",
            pad(field("name"), 22),
            field("current"),
            field("available")
        );
    }
    println!();
    println!("Install with `fwupdmgr update`; EpochShell does not flash firmware for you.");
}

fn toggle(ctx: &Context, action: ToggleAction) -> Result<()> {
    let ToggleAction::StayAwake { state, reason } = action;
    let mut params = json!({});
    let object = params.as_object_mut().expect("params is an object");
    // Omitting `enabled` is what asks the backend to flip whatever it currently is, which is what
    // a keybinding wants; naming a state is for scripts that need it definitely on or off.
    if let Some(state) = &state {
        object.insert("enabled".into(), json!(state == "on"));
    }
    if let Some(reason) = &reason {
        object.insert("reason".into(), json!(reason));
    }

    if ctx.dry_run {
        println!("epochoxide api system.setStayAwake --params '{params}'");
        return Ok(());
    }

    let data = ctx.oxide()?.api("system.setStayAwake", params)?;
    ctx.format.emit(&data, || {
        let enabled = data
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !enabled {
            println!("stay-awake off");
            return;
        }
        let reason = data.get("reason").and_then(Value::as_str).unwrap_or("");
        println!("stay-awake on ({reason})");
    });
    Ok(())
}

fn power(ctx: &Context, action: PowerAction) -> Result<()> {
    let PowerAction::Profile = action;
    if ctx.dry_run {
        println!("epochoxide api system.power");
        return Ok(());
    }
    let data = ctx.oxide()?.api("system.power", json!({}))?;
    ctx.format.emit(&data, || {
        let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default();
        if data.get("available").and_then(Value::as_bool) != Some(true) {
            println!("{}", text("reason"));
            return;
        }
        let profile = text("profile");
        println!(
            "{} {}",
            pad("profile", 12),
            if profile.is_empty() {
                "unknown".to_string()
            } else {
                profile.to_string()
            }
        );
        println!("{} {}", pad("governor", 12), text("governor"));
        let preference = text("energy_preference");
        if !preference.is_empty() {
            println!("{} {preference}", pad("energy", 12));
        }
        match data.get("turbo").and_then(Value::as_bool) {
            Some(on) => println!("{} {}", pad("turbo", 12), if on { "on" } else { "off" }),
            // A machine with no turbo knob is not a machine with turbo off.
            None => {}
        }
        println!("{} {}", pad("driver", 12), text("driver"));
        let manager = text("manager");
        println!(
            "{} {}",
            pad("managed by", 12),
            if manager.is_empty() {
                "nothing"
            } else {
                manager
            }
        );
        if let Some(platform) = data.get("platform_profile").and_then(Value::as_str) {
            println!("{} {platform}", pad("platform", 12));
        }
        // Switching is a separate problem: whatever is managing the CPU would put its own decision
        // back seconds later unless asked through its own override.
        if data.get("can_switch").and_then(Value::as_bool) != Some(true) {
            println!();
            println!("Read-only: switching goes through whatever daemon is managing the CPU.");
        }
    });
    Ok(())
}

fn nix(ctx: &Context, action: NixAction) -> Result<()> {
    let (method, params) = match &action {
        NixAction::Status => ("nix.status", json!({})),
        NixAction::Check => ("nix.check", json!({})),
        NixAction::Update => ("nix.update", json!({})),
        NixAction::Hosts => ("nix.hosts", json!({})),
        NixAction::Rebuild { host } => (
            "nix.rebuild",
            match host {
                Some(host) => json!({ "host": host }),
                None => json!({}),
            },
        ),
    };

    if ctx.dry_run {
        println!("epochoxide api {method} --params '{params}'");
        return Ok(());
    }

    let mut client = ctx.oxide()?;
    // A check resolves every input over the network, and there is no sensible bound on how long
    // that takes on a slow connection.
    if matches!(action, NixAction::Check) {
        client.set_read_timeout(None)?;
    }
    let data = client.api(method, params)?;

    match action {
        NixAction::Update | NixAction::Rebuild { .. } => {
            ctx.format.emit(&data, || {
                let command = data.get("command").and_then(Value::as_str).unwrap_or("");
                println!("running in a terminal: {command}");
            });
        }
        NixAction::Hosts => {
            ctx.format.emit(&data, || {
                let empty = Vec::new();
                let hosts = data.as_array().unwrap_or(&empty);
                if hosts.is_empty() {
                    println!("no hosts");
                    return;
                }
                for host in hosts {
                    let name = host.get("name").and_then(Value::as_str).unwrap_or("");
                    let rebuild = host.get("rebuild").and_then(Value::as_str).unwrap_or("");
                    println!(
                        "{} {}",
                        pad(name, 14),
                        if rebuild.is_empty() {
                            "(no rebuild command)"
                        } else {
                            rebuild
                        }
                    );
                }
            });
        }
        NixAction::Status | NixAction::Check => nix_status(ctx, &data),
    }
    Ok(())
}

fn nix_status(ctx: &Context, data: &Value) {
    ctx.format.emit(data, || {
        let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag = |key: &str| data.get(key).and_then(Value::as_bool).unwrap_or(false);
        let number = |key: &str| data.get(key).and_then(Value::as_u64).unwrap_or(0);

        if !flag("configured") {
            println!("no flake configured (set nix_flake in EpochOxide's config)");
            return;
        }
        println!("{} {}", pad("flake", 10), text("flake"));
        if !flag("available") {
            println!("{} {}", pad("status", 10), text("reason"));
            return;
        }
        if let Some(error) = data.get("error").and_then(Value::as_str) {
            println!("{} {error}", pad("error", 10));
        }
        let checked = number("checked_at");
        println!(
            "{} {}",
            pad("checked", 10),
            if flag("checking") {
                "checking now".to_string()
            } else if checked == 0 {
                "never".to_string()
            } else {
                format!("{} ago", ago(checked))
            }
        );
        println!("{} {} ago", pad("locked", 10), ago(number("locked_at")));

        let empty = Vec::new();
        let inputs = data
            .get("inputs")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let updates = number("updates");
        println!(
            "{} {}",
            pad("updates", 10),
            match (updates, inputs.len()) {
                (_, 0) => "nothing checked yet".to_string(),
                (0, total) => format!("none, {total} inputs are current"),
                (1, total) => format!("1 of {total} inputs can move"),
                (some, total) => format!("{some} of {total} inputs can move"),
            }
        );
        // Only what can move is listed: a wall of unchanged inputs buries the answer.
        for input in inputs.iter().filter(|input| {
            input
                .get("update_available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        }) {
            let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default();
            println!(
                "  {} {} -> {}  {}",
                pad(field("name"), 20),
                short_rev(field("current_rev")),
                short_rev(field("latest_rev")),
                field("source")
            );
        }
    });
}

/// A revision as people quote it. Comparison always uses the whole thing.
fn short_rev(rev: &str) -> String {
    if rev.is_empty() {
        "-".to_string()
    } else {
        rev.chars().take(7).collect()
    }
}

/// A unix timestamp as "3 hours", for a line that already says what it is measuring.
fn ago(timestamp: u64) -> String {
    if timestamp == 0 {
        return "never".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let seconds = now.saturating_sub(timestamp);
    let plural = |value: u64, unit: &str| {
        if value == 1 {
            format!("1 {unit}")
        } else {
            format!("{value} {unit}s")
        }
    };
    match seconds {
        0..60 => "moments".to_string(),
        60..3600 => plural(seconds / 60, "minute"),
        3600..86400 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86400, "day"),
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
