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
        LauncherAction::Keybinds => {
            let reply = ctx.call("launcher", "openKeybinds", &[])?;
            report_action(ctx, reply, "launcher opened on keybinds");
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
