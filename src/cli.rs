use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell as CompletionShell;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "epochctl",
    version,
    about = "Control CLI for the Epoch desktop shell",
    long_about = "Control the running Epoch shell and query its backend.\n\n\
                  Shell commands go through Quickshell IPC; search and activation talk to \
                  EpochOxide over its Unix socket. Use `epochctl doctor` when something is not \
                  responding.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Clone)]
pub struct GlobalArgs {
    /// Quickshell config directory [env: EPOCHSHELL_CONFIG_DIR] [default: ~/.config/epochshell]
    #[arg(long, short = 'C', global = true, value_name = "DIR")]
    pub config: Option<PathBuf>,

    /// EpochOxide socket [env: EPOCHOXIDE_SOCKET] [default: $XDG_RUNTIME_DIR/epochoxide.sock]
    #[arg(long, global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    /// Target a specific shell instance id (a unique substring is enough)
    #[arg(long, short = 'i', global = true, value_name = "ID")]
    pub instance: Option<String>,

    /// Target the shell instance with this process id
    #[arg(long, global = true, value_name = "PID")]
    pub pid: Option<u32>,

    /// Use the most recently launched instance instead of the oldest
    #[arg(long, short = 'n', global = true)]
    pub newest: bool,

    /// Do not filter instances by the display they were launched on
    #[arg(long, global = true)]
    pub any_display: bool,

    /// Emit JSON instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    /// Print the underlying command instead of running it
    #[arg(long, global = true)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Control the application launcher
    Launcher {
        #[command(subcommand)]
        action: LauncherAction,
    },
    /// Open, close, and inspect shell panels
    Panel {
        #[command(subcommand)]
        action: PanelAction,
    },
    /// Inspect and reload the shell process
    Shell {
        #[command(subcommand)]
        action: ShellAction,
    },
    /// Check that the shell is responding (alias for `shell ping`)
    Ping,
    /// Reload the shell configuration (alias for `shell reload`)
    Reload {
        /// Tear down and rebuild the QML engine, dropping notification and polkit registration
        #[arg(long)]
        hard: bool,
    },
    /// Take screenshots
    Capture {
        #[command(subcommand)]
        action: CaptureAction,
    },
    /// Search the EpochOxide providers
    Search(SearchArgs),
    /// Activate a result returned by `search`
    Activate(ActivateArgs),
    /// List EpochOxide providers
    Providers,
    /// List the entries of a custom EpochOxide menu
    Menu {
        /// Menu name, as configured in EpochOxide's menus_dir
        name: String,
    },
    /// Diagnose the shell, the backend, and this CLI's view of them
    Doctor,
    /// Generate a shell completion script
    Completions {
        /// Shell to generate completions for
        shell: CompletionShell,
    },
}

#[derive(Subcommand)]
pub enum LauncherAction {
    /// Toggle the launcher
    Toggle,
    /// Open the launcher
    Open,
    /// Close the launcher
    Close,
    /// Open the launcher scoped to one provider
    Provider {
        /// Provider name, for example files, clipboard, windows, keybinds
        name: String,
    },
    /// Report whether the launcher is open
    Status,
}

#[derive(Subcommand)]
pub enum PanelAction {
    /// Toggle a panel
    Toggle {
        /// Panel name, for example audio, wifi, bluetooth, tailscale
        name: String,
    },
    /// Open a panel
    Open { name: String },
    /// Close a panel
    Close { name: String },
    /// Report whether a panel is open
    Status { name: String },
    /// List every panel the shell exposes
    List,
    /// Close every open panel
    CloseAll,
}

#[derive(Subcommand)]
pub enum ShellAction {
    /// Check that the shell is responding
    Ping,
    /// Show instance, screen, and backend information
    Info,
    /// Reload the shell configuration
    Reload {
        /// Tear down and rebuild the QML engine
        #[arg(long)]
        hard: bool,
    },
    /// List the IPC targets the running shell exposes
    Targets,
}

#[derive(Subcommand)]
pub enum CaptureAction {
    /// Take a screenshot
    Screenshot(ScreenshotArgs),
    /// Show where screenshots land and which capture tools are installed
    Status,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ScreenshotMode {
    /// Drag out a rectangle
    Region,
    /// The focused window, or one you click with --select
    Window,
    /// One whole monitor
    #[value(alias = "screen", alias = "monitor", alias = "output")]
    Fullscreen,
    /// Every monitor, as one image
    All,
}

impl ScreenshotMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Window => "window",
            Self::Fullscreen => "fullscreen",
            Self::All => "all",
        }
    }
}

#[derive(Args)]
pub struct ScreenshotArgs {
    /// What to capture
    #[arg(value_enum, default_value_t = ScreenshotMode::Region)]
    pub mode: ScreenshotMode,

    /// Monitor to capture, for fullscreen; defaults to the focused one
    #[arg(long, short = 'o', value_name = "NAME")]
    pub output: Option<String>,

    /// Click the window to capture instead of taking the focused one
    #[arg(long)]
    pub select: bool,

    /// Include the mouse pointer
    #[arg(long)]
    pub cursor: bool,

    /// Wait this many seconds before capturing, after any selection is made
    #[arg(long, short = 'd', value_name = "SECONDS", default_value_t = 0.0)]
    pub delay: f64,

    /// Save into this directory instead of the configured one
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Leave the clipboard alone
    #[arg(long)]
    pub no_copy: bool,

    /// Do not keep the file; copy the shot and leave it in the cache
    #[arg(long)]
    pub no_save: bool,

    /// Do not show a notification
    #[arg(long)]
    pub no_notify: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    /// Text to search for
    #[arg(default_value = "")]
    pub query: String,

    /// Provider to search; repeat for several, or omit to search them all
    #[arg(long, short = 'p', value_name = "NAME")]
    pub provider: Vec<String>,

    /// Maximum results
    #[arg(long, short = 'l', default_value_t = 20)]
    pub limit: usize,

    /// Match exactly instead of fuzzily
    #[arg(long)]
    pub exact: bool,
}

#[derive(Args)]
pub struct ActivateArgs {
    /// Provider that owns the item
    pub provider: String,
    /// Item identifier, as returned by `search --json`
    pub identifier: String,
    /// Action to run; defaults to the provider's first action
    #[arg(long, short = 'a', default_value = "")]
    pub action: String,
    /// Extra arguments passed to the action
    #[arg(long, default_value = "")]
    pub arguments: String,
}
