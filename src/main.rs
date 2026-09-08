mod cli;
mod config;
mod doctor;
mod output;
mod oxide;
mod qs;
mod run;

use clap::{CommandFactory, Parser};
use cli::{Cli, Command};
use config::Config;
use output::Format;
use run::Context;
use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Completions are generated from the parser itself and never touch the shell or the backend,
    // so they are handled before anything tries to resolve a config directory or a socket.
    if let Command::Completions { shell } = cli.command {
        let mut command = Cli::command();
        let name = command.get_name().to_string();
        clap_complete::generate(shell, &mut command, name, &mut io::stdout());
        return ExitCode::SUCCESS;
    }

    let global = cli.global.clone();
    let config = match Config::resolve(
        global.config,
        global.socket,
        global.instance,
        global.pid,
        global.newest,
        global.any_display,
    ) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("epochctl: {err}");
            return ExitCode::from(2);
        }
    };

    let ctx = Context {
        config,
        format: Format::new(global.json),
        dry_run: global.dry_run,
    };

    match run::dispatch(&ctx, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if ctx.format.is_json() {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": err.to_string(),
                    "exit_code": err.exit_code(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_default()
                );
            } else {
                eprintln!("epochctl: {err}");
            }
            ExitCode::from(err.exit_code() as u8)
        }
    }
}
