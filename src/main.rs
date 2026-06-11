mod camera;
mod config;
mod discovery;
mod init;
mod rtsp_grab;
mod style;
mod tui;
mod vendors;

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use camera::{MotionStatus, PtzDirection, StreamQuality};
use clap::{CommandFactory, Parser, Subcommand};
use std::io::IsTerminal as _;

#[derive(Debug, Clone, clap::ValueEnum, PartialEq)]
enum OutputFormat {
    Auto,
    Text,
    Json,
}

#[derive(Parser)]
#[command(name = "ipcam", about = "Manage IP cameras from the command line")]
struct Cli {
    /// Path to config file (default: ~/.config/ipcam/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Output format (auto emits JSON when stdout is not a TTY)
    #[arg(
        long = "output",
        short = 'o',
        visible_alias = "format",
        global = true,
        default_value = "auto"
    )]
    format: OutputFormat,

    /// Output as JSON (deprecated: use --output json)
    #[arg(long, global = true, hide = true)]
    json: bool,

    /// Suppress informational output
    #[arg(long, short, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Command,
}

impl Cli {
    fn effective_json(&self) -> bool {
        self.format == OutputFormat::Json
            || self.json
            || (!matches!(self.format, OutputFormat::Text) && !std::io::stdout().is_terminal())
    }
}

#[derive(Subcommand)]
enum Command {
    /// List configured cameras
    List {
        /// Maximum number of items to return
        #[arg(long)]
        limit: Option<u64>,

        /// Number of items to skip
        #[arg(long)]
        offset: Option<u64>,

        /// Comma-separated list of fields to include
        #[arg(long)]
        fields: Option<String>,
    },

    /// Get camera info
    Info {
        /// Camera name from config
        camera: String,
    },

    /// Capture a snapshot from a camera
    Snapshot {
        /// Camera name from config (omit with --all to snapshot all cameras)
        camera: Option<String>,

        /// Output file path (default: <camera>_<timestamp>.jpg)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Snapshot all configured cameras (alias for snapshot-all)
        #[arg(long, conflicts_with_all = ["camera", "output"])]
        all: bool,

        /// Capture snapshots from all cameras and assemble into a single tiled grid image
        #[arg(long, conflicts_with_all = ["camera", "output"])]
        grid: bool,

        /// Directory to save snapshots when using --all or --every (default: current directory)
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Capture repeatedly at this interval (e.g. "30s", "5m", "1h"); saves timestamped files
        #[arg(long, conflicts_with_all = ["all", "grid"])]
        every: Option<String>,

        /// Stamp the camera name and timestamp onto the image using ffmpeg drawtext
        #[arg(long)]
        label: bool,

        /// Display the snapshot in the terminal after saving
        #[arg(long)]
        preview: bool,
    },

    /// Open a live RTSP stream from a camera
    Live {
        /// Camera name from config
        camera: String,

        /// Stream quality
        #[arg(long, default_value = "main")]
        quality: StreamQuality,

        /// Open in a separate window using ffplay instead of inline in the terminal
        #[arg(long)]
        window: bool,
    },

    /// Preview a camera snapshot in the terminal
    Preview {
        /// Camera name from config
        camera: String,

        /// Use sub-stream (lower quality, faster)
        #[arg(long)]
        sub: bool,
    },

    /// Capture snapshots from all configured cameras in parallel
    SnapshotAll {
        /// Directory to save snapshots (default: current directory)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },

    /// Print the RTSP stream URL for a camera
    Stream {
        /// Camera name from config
        camera: String,

        /// Stream quality
        #[arg(long, default_value = "main")]
        quality: StreamQuality,

        /// Pipe stream to file using ffmpeg
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Recording duration in seconds (used with --output)
        #[arg(short, long, default_value = "10")]
        duration: u64,
    },

    /// Record a clip from a camera
    Record {
        /// Camera name from config
        camera: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Duration in seconds
        #[arg(short, long, default_value = "30")]
        duration: u64,
    },

    /// Capture a timelapse from a camera
    Timelapse {
        /// Camera name from config
        camera: String,

        /// Interval between snapshots (e.g. "30s", "5m")
        #[arg(long, default_value = "30s")]
        interval: String,

        /// Total duration of the capture session (e.g. "1h", "30m")
        #[arg(long, default_value = "1h")]
        duration: String,

        /// Output MP4 file path
        #[arg(short, long, default_value = "timelapse.mp4")]
        output: PathBuf,

        /// Also keep individual frames in this directory
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// Manage the config file
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Discover cameras on the network and add them to the config
    Discover {
        /// How long to wait for responses (seconds)
        #[arg(short, long, default_value = "5")]
        timeout: u64,

        /// Only list discovered cameras, don't add them
        #[arg(long)]
        no_add: bool,

        /// Scan specific subnet(s) via TCP probes (e.g. 10.10.20.0/24). Can be repeated.
        #[arg(long)]
        subnet: Vec<String>,
    },

    /// Watch for motion and doorbell events
    Events {
        /// Camera name from config
        camera: String,

        /// Poll continuously and print events as they happen
        #[arg(short, long)]
        watch: bool,
    },

    /// Check which cameras are online
    Status {
        /// Camera name (omit to check all cameras)
        camera: Option<String>,

        /// Maximum number of items to return
        #[arg(long)]
        limit: Option<u64>,

        /// Number of items to skip
        #[arg(long)]
        offset: Option<u64>,

        /// Comma-separated list of fields to include
        #[arg(long)]
        fields: Option<String>,
    },

    /// Control pan/tilt/zoom on a camera
    Ptz {
        /// Camera name from config
        camera: String,

        /// PTZ action: left, right, up, down, stop, preset
        action: PtzAction,

        /// Preset number (required for "preset" action)
        preset: Option<u32>,

        /// Movement speed (1-9, default 5)
        #[arg(short, long, default_value = "5", value_parser = clap::value_parser!(u8).range(1..=9))]
        speed: u8,
    },

    /// Generate shell completion scripts
    ///
    /// Print a completion script to stdout and install it for your shell:
    ///
    ///   ipcam completions zsh  > ~/.zfunc/_ipcam
    ///   ipcam completions bash > /etc/bash_completion.d/ipcam
    ///   ipcam completions fish > ~/.config/fish/completions/ipcam.fish
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },

    /// Interactively set up the config file from discovered cameras
    Init {
        /// Non-interactive: auto-generate config from discovered cameras using defaults
        #[arg(long)]
        auto: bool,
    },

    /// Continuously monitor camera health and print status changes
    Watch {
        /// How often to poll cameras (e.g. "30s", "5m", "1h")
        #[arg(long, default_value = "30s")]
        interval: String,

        /// Shell command to run on status change; receives CAMERA_NAME, CAMERA_HOST,
        /// CAMERA_STATUS (online/offline), and CAMERA_DETAIL as environment variables
        #[arg(long)]
        exec: Option<String>,
    },

    /// Test a camera's configuration end-to-end (network, RTSP, snapshot)
    Test {
        /// Camera name from config (omit to test all cameras in parallel)
        camera: Option<String>,
    },

    /// Live TUI showing all camera statuses
    Tui {
        /// Refresh interval in seconds
        #[arg(short, long, default_value = "5")]
        interval: u64,
    },

    /// Rename a camera in the config file
    Rename {
        /// Current camera name
        old_name: String,
        /// New camera name
        new_name: String,
    },

    /// Add a camera to the config manually
    Add {
        /// IP address of the camera
        #[arg(long)]
        host: String,

        /// Camera name (default: auto-generated from last octet)
        #[arg(long)]
        name: Option<String>,

        /// Camera type
        #[arg(long, value_name = "TYPE")]
        r#type: CameraTypeArg,

        /// Username (default: admin)
        #[arg(long, default_value = "admin")]
        username: String,

        /// Password
        #[arg(long)]
        password: Option<String>,

        /// RTSP port (default: 554)
        #[arg(long, default_value = "554")]
        rtsp_port: u16,

        /// go2rtc stream name
        #[arg(long)]
        go2rtc_stream: Option<String>,
    },

    /// Remove a camera from the config
    Remove {
        /// Camera name to remove
        name: String,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Print the machine-readable schema for this tool (clispec v0.2)
    Schema {
        /// Filter to a specific command name
        command: Option<String>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum PtzAction {
    Left,
    Right,
    Up,
    Down,
    Stop,
    Preset,
    ZoomIn,
    ZoomOut,
    Home,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum CameraTypeArg {
    Tapo,
    Reolink,
}

impl From<CameraTypeArg> for config::CameraType {
    fn from(arg: CameraTypeArg) -> Self {
        match arg {
            CameraTypeArg::Tapo => config::CameraType::Tapo,
            CameraTypeArg::Reolink => config::CameraType::Reolink,
        }
    }
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config file path and status (default)
    Path,

    /// Open the config file in $EDITOR (or $VISUAL, fallback to vim)
    Edit,

    /// Print the current config with passwords masked
    Show,

    /// Validate the config and check camera connectivity
    Check,
}

#[tokio::main]
async fn main() {
    // Use try_parse so clap errors pass through our structured error handler.
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Print clap's human-readable message for TTY users, then emit
            // a structured error envelope as the last line of stderr so that
            // automated consumers can parse it without grepping prose.
            let exit_code = e.exit_code();
            e.print().ok();
            let envelope = serde_json::json!({
                "error": {
                    "kind": "error",
                    "message": "unrecognized argument or subcommand",
                }
            });
            eprintln!("{}", envelope);
            std::process::exit(exit_code);
        }
    };

    let suppress = cli.effective_json() || cli.quiet;

    if suppress {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("ipcam=warn".parse().unwrap()),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("ipcam=info".parse().unwrap()),
            )
            .init();
    }

    let json_mode = cli.effective_json();
    if let Err(err) = run_with(cli).await {
        print_error(&err, json_mode);
        std::process::exit(1);
    }
}

fn error_kind(err: &anyhow::Error) -> &'static str {
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("connection refused")
        || msg.contains("timed out")
        || msg.contains("no route to host")
        || msg.contains("network unreachable")
    {
        "network_error"
    } else if msg.contains("ffmpeg")
        && (msg.contains("no such file or directory") || msg.contains("os error 2"))
    {
        "dependency_missing"
    } else if msg.contains("401")
        || msg.contains("403")
        || msg.contains("unauthorized")
        || msg.contains("authentication")
        || msg.contains("wrong password")
    {
        "auth"
    } else if msg.contains("no cameras configured") || msg.contains("unconfigured") {
        "not_configured"
    } else if msg.contains("no such camera")
        || msg.contains("camera not found")
        || msg.contains("not found in config")
        || (msg.contains("not found") && msg.contains("camera"))
    {
        "not_found"
    } else if msg.contains("already exists") {
        "conflict"
    } else {
        "error"
    }
}

fn print_error(err: &anyhow::Error, json_mode: bool) {
    let kind = error_kind(err);
    let msg = redact_url(&err.to_string());

    let hint = {
        let lower = format!("{err:#}").to_lowercase();
        if lower.contains("connection refused")
            || lower.contains("timed out")
            || lower.contains("no route to host")
            || lower.contains("network unreachable")
        {
            Some("Check that the camera is powered on and connected to your network.")
        } else if lower.contains("ffmpeg")
            && (lower.contains("no such file or directory") || lower.contains("os error 2"))
        {
            Some(
                "ffmpeg is not installed. Install it with: brew install ffmpeg (macOS) or apt install ffmpeg (Linux)",
            )
        } else if lower.contains("401")
            || lower.contains("403")
            || lower.contains("unauthorized")
            || lower.contains("authentication")
            || lower.contains("wrong password")
        {
            Some("Check the username and password in your config file.")
        } else {
            None
        }
    };

    if !json_mode {
        eprintln!("Error: {}", msg);

        let mut source = err.source();
        while let Some(cause) = source {
            eprintln!("  caused by: {}", redact_url(&cause.to_string()));
            source = cause.source();
        }

        if hint.is_some() {
            eprintln!();
        }
    }

    let envelope = if let Some(h) = hint {
        serde_json::json!({
            "error": {
                "kind": kind,
                "message": msg,
                "hint": h,
            }
        })
    } else {
        serde_json::json!({
            "error": {
                "kind": kind,
                "message": msg,
            }
        })
    };
    eprintln!("{}", envelope);

    if !json_mode
        && (std::env::var("RUST_BACKTRACE").as_deref() == Ok("1")
            || std::env::var("RUST_BACKTRACE").as_deref() == Ok("full"))
    {
        let bt = err.backtrace();
        let bt_str = bt.to_string();
        if !bt_str.is_empty() && bt_str != "disabled backtrace" {
            eprintln!();
            eprintln!("{bt_str}");
        }
    }
}

fn apply_fields_filter(
    items: Vec<serde_json::Value>,
    fields: Option<&str>,
) -> Vec<serde_json::Value> {
    match fields {
        None => items,
        Some(f) => {
            let field_names: Vec<&str> = f.split(',').map(|s| s.trim()).collect();
            items
                .into_iter()
                .map(|mut item| {
                    if let Some(obj) = item.as_object_mut() {
                        let keys: Vec<String> = obj.keys().cloned().collect();
                        for k in keys {
                            if !field_names.contains(&k.as_str()) {
                                obj.remove(&k);
                            }
                        }
                    }
                    item
                })
                .collect()
        }
    }
}

fn build_schema() -> serde_json::Value {
    serde_json::json!({
        "clispec": "0.2",
        "name": "ipcam",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Manage IP cameras from the command line",
        "global_args": [
            {"name": "--output", "type": "string", "enum": ["auto", "text", "json"], "default": "auto", "description": "Output format; auto emits JSON when stdout is not a TTY (alias: --format, short: -o)"},
            {"name": "--quiet", "type": "boolean", "default": false, "description": "Suppress informational output"},
            {"name": "--config", "type": "path", "required": false, "description": "Path to config file"}
        ],
        "commands": [
            {
                "name": "list",
                "description": "List configured cameras",
                "mutating": false,
                "args": [
                    {"name": "--limit", "type": "integer", "required": false, "description": "Maximum number of items to return"},
                    {"name": "--offset", "type": "integer", "required": false, "description": "Number of items to skip"},
                    {"name": "--fields", "type": "string", "required": false, "description": "Comma-separated list of fields to include in output"}
                ],
                "output_fields": [
                    {"name": "name", "type": "string"},
                    {"name": "type", "type": "string"},
                    {"name": "host", "type": "string"}
                ]
            },
            {
                "name": "info",
                "description": "Get camera info",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": true, "description": "Camera name from config"}
                ],
                "output_fields": [
                    {"name": "name", "type": "string"},
                    {"name": "host", "type": "string"},
                    {"name": "model", "type": "string"},
                    {"name": "firmware", "type": "string"}
                ]
            },
            {
                "name": "snapshot",
                "description": "Capture a snapshot from a camera",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": false, "description": "Camera name from config"},
                    {"name": "--output", "type": "path", "required": false, "description": "Output file path"},
                    {"name": "--all", "type": "boolean", "default": false, "description": "Snapshot all cameras"},
                    {"name": "--grid", "type": "boolean", "default": false, "description": "Assemble snapshots into a tiled grid"},
                    {"name": "--output-dir", "type": "path", "required": false, "description": "Directory for saving snapshots"},
                    {"name": "--every", "type": "string", "required": false, "description": "Capture repeatedly at this interval"},
                    {"name": "--label", "type": "boolean", "default": false, "description": "Stamp camera name and timestamp on image"},
                    {"name": "--preview", "type": "boolean", "default": false, "description": "Display snapshot in terminal after saving"}
                ],
                "output_fields": [
                    {"name": "camera", "type": "string"},
                    {"name": "file", "type": "string"},
                    {"name": "size_bytes", "type": "integer"},
                    {"name": "timestamp", "type": "string"}
                ]
            },
            {
                "name": "status",
                "description": "Check which cameras are online",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": false, "description": "Camera name (omit to check all)"},
                    {"name": "--limit", "type": "integer", "required": false, "description": "Maximum number of items to return"},
                    {"name": "--offset", "type": "integer", "required": false, "description": "Number of items to skip"},
                    {"name": "--fields", "type": "string", "required": false, "description": "Comma-separated list of fields to include"}
                ],
                "output_fields": [
                    {"name": "camera", "type": "string"},
                    {"name": "online", "type": "boolean"},
                    {"name": "detail", "type": "string"},
                    {"name": "latency_ms", "type": "integer"}
                ]
            },
            {
                "name": "add",
                "description": "Add a camera to the config manually",
                "mutating": true,
                "args": [
                    {"name": "--host", "type": "string", "required": true, "description": "IP address of the camera"},
                    {"name": "--name", "type": "string", "required": false, "description": "Camera name"},
                    {"name": "--type", "type": "string", "required": true, "enum": ["tapo", "reolink"], "description": "Camera type"},
                    {"name": "--username", "type": "string", "default": "admin", "description": "Username"},
                    {"name": "--password", "type": "string", "required": false, "description": "Password"},
                    {"name": "--rtsp-port", "type": "integer", "default": 554, "description": "RTSP port"},
                    {"name": "--go2rtc-stream", "type": "string", "required": false, "description": "go2rtc stream name"}
                ]
            },
            {
                "name": "remove",
                "description": "Remove a camera from the config",
                "mutating": true,
                "args": [
                    {"name": "name", "type": "string", "required": true, "description": "Camera name to remove"},
                    {"name": "--yes", "type": "boolean", "default": false, "description": "Skip confirmation prompt"}
                ]
            },
            {
                "name": "rename",
                "description": "Rename a camera in the config file",
                "mutating": true,
                "args": [
                    {"name": "old_name", "type": "string", "required": true},
                    {"name": "new_name", "type": "string", "required": true}
                ]
            },
            {
                "name": "ptz",
                "description": "Control pan/tilt/zoom on a camera",
                "mutating": true,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "action", "type": "string", "required": true, "enum": ["left", "right", "up", "down", "stop", "preset", "zoom-in", "zoom-out", "home"]},
                    {"name": "preset", "type": "integer", "required": false},
                    {"name": "--speed", "type": "integer", "default": 5}
                ]
            },
            {
                "name": "record",
                "description": "Record a clip from a camera",
                "mutating": true,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--output", "type": "path", "required": false},
                    {"name": "--duration", "type": "integer", "default": 30}
                ]
            },
            {
                "name": "timelapse",
                "description": "Capture a timelapse from a camera",
                "mutating": true,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--interval", "type": "string", "default": "30s"},
                    {"name": "--duration", "type": "string", "default": "1h"},
                    {"name": "--output", "type": "path", "default": "timelapse.mp4"},
                    {"name": "--output-dir", "type": "path", "required": false}
                ]
            },
            {
                "name": "discover",
                "description": "Discover cameras on the network",
                "mutating": false,
                "args": [
                    {"name": "--timeout", "type": "integer", "default": 5},
                    {"name": "--no-add", "type": "boolean", "default": false},
                    {"name": "--subnet", "type": "string[]", "required": false}
                ]
            },
            {
                "name": "events",
                "description": "Watch for motion and doorbell events",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--watch", "type": "boolean", "default": false}
                ]
            },
            {
                "name": "config",
                "description": "Manage the config file",
                "mutating": false,
                "subcommands": [
                    {"name": "path", "description": "Print config file path", "mutating": false},
                    {"name": "edit", "description": "Open config in editor", "mutating": true},
                    {"name": "show", "description": "Print config with passwords masked", "mutating": false},
                    {"name": "check", "description": "Validate config and check connectivity", "mutating": false}
                ]
            },
            {
                "name": "completions",
                "description": "Generate shell completion scripts",
                "mutating": false,
                "args": [
                    {"name": "shell", "type": "string", "required": true, "enum": ["bash", "zsh", "fish", "powershell", "elvish"]}
                ]
            },
            {
                "name": "init",
                "description": "Interactively set up the config file",
                "mutating": true,
                "args": [
                    {"name": "--auto", "type": "boolean", "default": false, "description": "Non-interactive: auto-generate config from discovered cameras"}
                ]
            },
            {
                "name": "watch",
                "description": "Continuously monitor camera health",
                "mutating": false,
                "args": [
                    {"name": "--interval", "type": "string", "default": "30s"},
                    {"name": "--exec", "type": "string", "required": false}
                ]
            },
            {
                "name": "test",
                "description": "Test a camera's configuration end-to-end",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": false}
                ]
            },
            {
                "name": "tui",
                "description": "Live TUI showing all camera statuses",
                "mutating": false,
                "args": [
                    {"name": "--interval", "type": "integer", "default": 5}
                ]
            },
            {
                "name": "live",
                "description": "Open a live RTSP stream from a camera",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--quality", "type": "string", "default": "main", "enum": ["main", "sub"]},
                    {"name": "--window", "type": "boolean", "default": false}
                ]
            },
            {
                "name": "preview",
                "description": "Preview a camera snapshot in the terminal",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--sub", "type": "boolean", "default": false}
                ]
            },
            {
                "name": "stream",
                "description": "Print the RTSP stream URL for a camera",
                "mutating": false,
                "args": [
                    {"name": "camera", "type": "string", "required": true},
                    {"name": "--quality", "type": "string", "default": "main"},
                    {"name": "--output", "type": "path", "required": false},
                    {"name": "--duration", "type": "integer", "default": 10}
                ]
            },
            {
                "name": "snapshot-all",
                "description": "Capture snapshots from all configured cameras in parallel",
                "mutating": false,
                "args": [
                    {"name": "--output-dir", "type": "path", "required": false}
                ]
            },
            {
                "name": "schema",
                "description": "Print the machine-readable schema for this tool (clispec v0.2)",
                "mutating": false,
                "args": [
                    {"name": "command", "type": "string", "required": false, "description": "Filter to a specific command name"}
                ]
            }
        ],
        "errors": [
            {"kind": "not_configured", "exit_code": 1, "retryable": false, "description": "No cameras configured; run ipcam init"},
            {"kind": "not_found", "exit_code": 1, "retryable": false, "description": "Camera not found in config"},
            {"kind": "auth", "exit_code": 1, "retryable": false, "description": "Authentication failed"},
            {"kind": "network_error", "exit_code": 1, "retryable": true, "description": "Camera unreachable on the network"},
            {"kind": "dependency_missing", "exit_code": 1, "retryable": false, "description": "Required external tool (e.g. ffmpeg) not installed"},
            {"kind": "conflict", "exit_code": 1, "retryable": false, "description": "Resource already exists with different configuration"},
            {"kind": "confirmation_required", "exit_code": 2, "retryable": false, "description": "Destructive operation requires --yes flag"},
            {"kind": "error", "exit_code": 1, "retryable": false, "description": "Unexpected error"}
        ]
    })
}

/// Redact credentials from an RTSP URL for safe logging/display.
/// Turns `rtsp://user:pass@host/path` into `rtsp://****:****@host/path`.
pub(crate) fn redact_url(url: &str) -> String {
    if let Some(at_pos) = url.find('@')
        && let Some(scheme_end) = url.find("://")
    {
        let prefix = &url[..scheme_end + 3];
        let after_at = &url[at_pos..];
        return format!("{}****:****{}", prefix, after_at);
    }
    url.to_string()
}

/// Parse a human-readable duration string like "30s", "5m", "1h", "2h30m".
fn parse_duration(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| {
        format!(
            "invalid duration '{}' — use formats like '30s', '5m', '1h', '2h30m'",
            s
        )
    })
}

async fn run_with(cli: Cli) -> Result<()> {
    if let Command::Completions { shell } = cli.command {
        let mut cmd = Cli::command();
        let bin_name = cmd.get_name().to_string();
        clap_complete::generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
        return Ok(());
    }

    // `init` does not require an existing config file.
    if let Command::Init { auto } = cli.command {
        return init::run_init(auto).await;
    }

    if let Command::Schema { command: filter } = &cli.command {
        let mut schema = build_schema();
        if let Some(cmd_filter) = filter
            && let Some(commands) = schema.get("commands").and_then(|c| c.as_array())
        {
            let filtered: Vec<_> = commands
                .iter()
                .filter(|c| c.get("name").and_then(|n| n.as_str()) == Some(cmd_filter.as_str()))
                .cloned()
                .collect();
            schema["commands"] = serde_json::json!(filtered);
        }
        println!("{}", serde_json::to_string_pretty(&schema)?);
        return Ok(());
    }

    config::Config::migrate_if_needed()?;

    // When no config file exists, give a helpful message for commands that need cameras.
    let needs_cameras = !matches!(
        cli.command,
        Command::Config { .. } | Command::Discover { .. } | Command::Schema { .. }
    );
    if needs_cameras && !config::Config::config_exists()? {
        let path = config::Config::config_path()?;
        bail!(
            "no cameras configured. Run `ipcam init` to set up your cameras, \
             or create a config at {}",
            path.display()
        );
    }

    let config = config::Config::load(cli.config.as_deref())?;
    let json = cli.effective_json();

    match cli.command {
        Command::List {
            limit,
            offset,
            fields,
        } => cmd_list(&config, json, limit, offset, fields.as_deref()),
        Command::Info { camera } => cmd_info(&config, &camera, json).await,
        Command::Snapshot {
            camera,
            output,
            all,
            grid,
            output_dir,
            every,
            label,
            preview,
        } => {
            if grid {
                cmd_snapshot_grid(&config, output_dir, label).await
            } else if all {
                cmd_snapshot_all(&config, output_dir, json, label).await
            } else if let Some(interval_str) = every {
                let name = camera
                    .ok_or_else(|| anyhow::anyhow!("camera name is required when using --every"))?;
                let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
                let interval = parse_duration(&interval_str)?;
                cmd_snapshot_watch(&config, &name, dir, interval).await
            } else {
                let name = camera.ok_or_else(|| {
                    anyhow::anyhow!(
                        "camera name is required (or use --all to snapshot all cameras)"
                    )
                })?;
                cmd_snapshot(&config, &name, output, label, preview, json).await
            }
        }
        Command::Live {
            camera,
            quality,
            window,
        } => cmd_live(&config, &camera, quality, window).await,
        Command::Preview { camera, sub: _ } => cmd_preview(&config, &camera).await,
        Command::SnapshotAll { output_dir } => {
            cmd_snapshot_all(&config, output_dir, json, false).await
        }
        Command::Stream {
            camera,
            quality,
            output,
            duration,
        } => cmd_stream(&config, &camera, quality, output, duration, json).await,
        Command::Record {
            camera,
            output,
            duration,
        } => cmd_record(&config, &camera, output, duration, json).await,
        Command::Timelapse {
            camera,
            interval,
            duration,
            output,
            output_dir,
        } => {
            let interval = parse_duration(&interval)?;
            let duration = parse_duration(&duration)?;
            cmd_timelapse(&config, &camera, interval, duration, output, output_dir).await
        }
        Command::Config { action } => match action.unwrap_or(ConfigAction::Path) {
            ConfigAction::Path => cmd_config_path(json),
            ConfigAction::Edit => cmd_config_edit(),
            ConfigAction::Show => cmd_config_show(json, cli.config.as_deref()),
            ConfigAction::Check => cmd_config_check(&config, json).await,
        },
        Command::Discover {
            timeout,
            no_add,
            subnet,
        } => {
            let subnets: Vec<&str> = subnet.iter().map(|s| s.as_str()).collect();
            if no_add {
                cmd_discover(timeout, json, &config, &subnets).await
            } else {
                cmd_add_discover(&config, cli.config.as_deref(), json, timeout, &subnets).await
            }
        }
        Command::Events { camera, watch } => cmd_events(&config, &camera, watch, json).await,
        Command::Status {
            camera,
            limit,
            offset,
            fields,
        } => {
            cmd_status(
                &config,
                camera.as_deref(),
                json,
                limit,
                offset,
                fields.as_deref(),
            )
            .await
        }
        Command::Ptz {
            camera,
            action,
            preset,
            speed,
        } => cmd_ptz(&config, &camera, action, preset, speed, json).await,
        Command::Watch { interval, exec } => {
            let interval = parse_duration(&interval)?;
            cmd_watch(&config, interval, exec.as_deref()).await
        }
        Command::Test { camera } => cmd_test(&config, camera.as_deref(), json).await,
        Command::Tui { interval } => tui::run_tui(&config, interval).await,
        Command::Rename { old_name, new_name } => {
            cmd_rename(&old_name, &new_name, cli.config.as_deref(), json)
        }
        Command::Add {
            host,
            name,
            r#type,
            username,
            password,
            rtsp_port,
            go2rtc_stream,
        } => cmd_add_direct(
            &host,
            name.as_deref(),
            r#type,
            &username,
            password.as_deref(),
            rtsp_port,
            go2rtc_stream.as_deref(),
            cli.config.as_deref(),
            json,
        ),
        Command::Remove { name, yes } => cmd_remove(&name, yes, cli.config.as_deref(), json),
        Command::Completions { .. } | Command::Init { .. } | Command::Schema { .. } => {
            unreachable!("handled before config load")
        }
    }
}

fn cmd_list(
    config: &config::Config,
    json: bool,
    limit: Option<u64>,
    offset: Option<u64>,
    fields: Option<&str>,
) -> Result<()> {
    if config.cameras.is_empty() {
        if json {
            println!("{}", serde_json::json!({"items": [], "total": 0}));
        } else {
            let config_path = config::Config::config_path()?;
            println!(
                "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
            );
            println!("Config file: {}", config_path.display());
        }
        return Ok(());
    }

    if json {
        let all_items: Vec<serde_json::Value> = config
            .cameras
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "type": c.camera_type.to_string(),
                    "host": c.host,
                })
            })
            .collect();
        let total = all_items.len();
        let offset_val = offset.unwrap_or(0) as usize;
        let items: Vec<_> = all_items.into_iter().skip(offset_val).collect();
        let items: Vec<_> = if let Some(lim) = limit {
            items.into_iter().take(lim as usize).collect()
        } else {
            items
        };
        let items = apply_fields_filter(items, fields);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "items": items,
                "total": total,
            }))?
        );
    } else {
        println!(
            "{}",
            style::bold(&format!("{:<20} {:<10} {}", "CAMERA", "TYPE", "HOST"))
        );
        println!("{}", style::dim(&"-".repeat(50)));
        for cam in &config.cameras {
            println!("{:<20} {:<10} {}", cam.name, cam.camera_type, cam.host);
        }
    }
    Ok(())
}

async fn cmd_info(config: &config::Config, name: &str, json: bool) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let info = cam.info().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("Name:     {}", info.name);
        println!("Host:     {}", info.host);
        if let Some(model) = &info.model {
            println!("Model:    {}", model);
        }
        if let Some(fw) = &info.firmware {
            println!("Firmware: {}", fw);
        }
    }
    Ok(())
}

/// Stamp the camera name and timestamp onto an image file using ffmpeg's drawtext filter.
///
/// The text is rendered near the bottom-left of the image with a black border for legibility.
/// The file is overwritten in-place.
async fn apply_label(path: &std::path::Path, camera_name: &str, timestamp: &str) -> Result<()> {
    // Colons in the drawtext text value must be escaped as `\:`.
    let escaped_ts = timestamp.replace(':', r"\:");
    let text = format!("{}  {}", camera_name, escaped_ts);
    let drawtext = format!(
        "drawtext=text='{}':fontsize=28:fontcolor=white:borderw=2:bordercolor=black:x=10:y=h-th-10",
        text
    );

    // Write to a temporary file alongside the original, then rename atomically.
    let tmp_path = path.with_extension("_label_tmp.jpg");

    let status = tokio::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(path)
        .args(["-vf", &drawtext, "-update", "1", "-y"])
        .arg(&tmp_path)
        .status()
        .await
        .context("failed to run ffmpeg — is it installed?")?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        eprintln!(
            "Warning: --label failed (ffmpeg drawtext filter unavailable; install ffmpeg with libfreetype support). Saving without label."
        );
        return Ok(());
    }

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming labelled image to {}", path.display()))?;

    Ok(())
}

async fn cmd_snapshot(
    config: &config::Config,
    name: &str,
    output: Option<PathBuf>,
    label: bool,
    preview: bool,
    json: bool,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    tracing::info!("capturing snapshot from '{}'...", name);
    let snapshot = cam.snapshot().await?;

    let ts = snapshot.timestamp.format("%Y%m%d_%H%M%S").to_string();

    let path = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}_{}.{}",
            snapshot.camera_name,
            ts,
            snapshot.format.extension()
        ))
    });

    std::fs::write(&path, &snapshot.data)?;

    if label {
        let display_ts = snapshot.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
        apply_label(&path, name, &display_ts).await?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "camera": name,
                "file": path.display().to_string(),
                "size_bytes": snapshot.data.len(),
                "timestamp": snapshot.timestamp.to_rfc3339(),
            })
        );
    } else {
        println!("Saved snapshot to {}", path.display());
    }

    if preview {
        print_image_preview(&path)?;
    }

    Ok(())
}

fn print_image_preview(path: &std::path::Path) -> Result<()> {
    let conf = viuer::Config {
        absolute_offset: false,
        ..Default::default()
    };
    viuer::print_from_file(path, &conf)
        .with_context(|| format!("failed to display image preview for {}", path.display()))?;
    Ok(())
}

async fn cmd_preview(config: &config::Config, name: &str) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    tracing::info!("capturing snapshot from '{}'...", name);
    let snapshot = cam.snapshot().await?;

    let temp_path = std::env::temp_dir().join(format!(
        "ipcam_preview_{}.{}",
        name,
        snapshot.format.extension()
    ));

    std::fs::write(&temp_path, &snapshot.data)?;
    print_image_preview(&temp_path)?;
    std::fs::remove_file(&temp_path).ok();

    Ok(())
}

async fn cmd_live(
    config: &config::Config,
    name: &str,
    quality: StreamQuality,
    window: bool,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let url = cam.rtsp_url(quality);

    if window {
        eprintln!("Opening live stream from '{}' in ffplay...", name);
        let status = tokio::process::Command::new("ffplay")
            .args([
                "-rtsp_transport",
                "tcp",
                "-loglevel",
                "error",
                "-window_title",
                &format!("ipcam - {}", name),
                &url,
            ])
            .status()
            .await
            .context("ffplay not found. It comes with ffmpeg — install ffmpeg first.")?;
        if !status.success() {
            bail!("ffplay exited with status {}", status);
        }
        return Ok(());
    }

    // Inline mode: grab frames via ffmpeg and display with viuer
    eprintln!("Live view of '{}' (press Ctrl+C to stop)...", name);
    let temp_path = std::env::temp_dir().join(format!("ipcam_live_{}.jpg", name));

    loop {
        let grab = async {
            let status = tokio::process::Command::new("ffmpeg")
                .args([
                    "-rtsp_transport",
                    "tcp",
                    "-loglevel",
                    "error",
                    "-y",
                    "-i",
                    &url,
                    "-frames:v",
                    "1",
                    "-update",
                    "1",
                ])
                .arg(temp_path.as_os_str())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            if let Ok(s) = status
                && s.success()
                && temp_path.exists()
            {
                // Clear screen and move cursor to top-left to overwrite previous frame
                print!("\x1b[H\x1b[2J");
                let _ = print_image_preview(&temp_path);
            }
        };

        tokio::select! {
            () = grab => {}
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    std::fs::remove_file(&temp_path).ok();
    println!("\nStopped.");
    Ok(())
}

async fn cmd_snapshot_all(
    config: &config::Config,
    output_dir: Option<PathBuf>,
    json: bool,
    label: bool,
) -> Result<()> {
    if config.cameras.is_empty() {
        if json {
            println!("{}", serde_json::json!({"successes": [], "failures": []}));
        } else {
            println!(
                "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
            );
        }
        return Ok(());
    }

    let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating output directory: {}", dir.display()))?;

    let futures: Vec<_> = config
        .cameras
        .iter()
        .map(|cam_config| {
            let name = cam_config.name.clone();
            let dir = dir.clone();
            let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref());
            async move {
                let cam = match cam {
                    Ok(c) => c,
                    Err(e) => return (name, Err(e)),
                };
                tracing::info!("capturing snapshot from '{}'...", name);
                match cam.snapshot().await {
                    Ok(snapshot) => {
                        let ts = snapshot.timestamp.format("%Y%m%d_%H%M%S").to_string();
                        let filename = format!(
                            "{}_{}.{}",
                            snapshot.camera_name,
                            ts,
                            snapshot.format.extension()
                        );
                        let path = dir.join(&filename);
                        match std::fs::write(&path, &snapshot.data) {
                            Ok(()) => {
                                if label {
                                    let display_ts =
                                        snapshot.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                                    if let Err(e) = apply_label(&path, &name, &display_ts).await {
                                        return (name, Err(e));
                                    }
                                }
                                (name, Ok(path))
                            }
                            Err(e) => (name, Err(anyhow::anyhow!("write failed: {e}"))),
                        }
                    }
                    Err(e) => (name, Err(e)),
                }
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    let mut successes: Vec<serde_json::Value> = Vec::new();
    let mut failures: Vec<serde_json::Value> = Vec::new();

    for (name, result) in &results {
        match result {
            Ok(path) => successes.push(serde_json::json!({
                "camera": name,
                "path": path.display().to_string(),
            })),
            Err(e) => failures.push(serde_json::json!({
                "camera": name,
                "error": e.to_string(),
            })),
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "successes": successes,
                "failures": failures,
            }))?
        );
    } else {
        for entry in &successes {
            println!(
                "  {} -> {}",
                entry["camera"].as_str().unwrap_or(""),
                entry["path"].as_str().unwrap_or(""),
            );
        }
        for entry in &failures {
            eprintln!(
                "  {} FAILED: {}",
                entry["camera"].as_str().unwrap_or(""),
                entry["error"].as_str().unwrap_or(""),
            );
        }
        println!();
        println!(
            "Summary: {} succeeded, {} failed",
            successes.len(),
            failures.len()
        );
    }

    if !failures.is_empty() {
        bail!("{} camera(s) failed to capture a snapshot", failures.len());
    }

    Ok(())
}

async fn cmd_snapshot_grid(
    config: &config::Config,
    output_dir: Option<PathBuf>,
    label: bool,
) -> Result<()> {
    if config.cameras.is_empty() {
        println!(
            "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
        );
        return Ok(());
    }

    let n = config.cameras.len();

    // Capture all cameras in parallel into a temp directory.
    let tmp_dir =
        std::env::temp_dir().join(format!("ipcam-grid-{}", chrono::Utc::now().timestamp()));
    std::fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("creating temp directory: {}", tmp_dir.display()))?;

    let futures: Vec<_> = config
        .cameras
        .iter()
        .enumerate()
        .map(|(idx, cam_config)| {
            let name = cam_config.name.clone();
            let tmp_dir = tmp_dir.clone();
            let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref());
            async move {
                let cam = match cam {
                    Ok(c) => c,
                    Err(e) => return (idx, name, Err(e)),
                };
                tracing::info!("capturing snapshot from '{}'...", name);
                match cam.snapshot().await {
                    Ok(snapshot) => {
                        let path = tmp_dir.join(format!("cam_{:04}.jpg", idx));
                        match std::fs::write(&path, &snapshot.data) {
                            Ok(()) => (idx, name, Ok(path)),
                            Err(e) => (idx, name, Err(anyhow::anyhow!("write failed: {e}"))),
                        }
                    }
                    Err(e) => (idx, name, Err(e)),
                }
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // Collect captured frames in camera order; report failures but continue.
    let mut frame_paths: Vec<Option<PathBuf>> = vec![None; n];
    let mut failures = 0usize;
    for (idx, name, result) in results {
        match result {
            Ok(path) => frame_paths[idx] = Some(path),
            Err(e) => {
                eprintln!("  {} FAILED: {}", name, e);
                failures += 1;
            }
        }
    }

    let successful: Vec<PathBuf> = frame_paths.into_iter().flatten().collect();
    if successful.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        bail!("all cameras failed to capture a snapshot");
    }

    let count = successful.len();
    let cols = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(cols);

    // For an incomplete last row, generate a black placeholder image via ffmpeg.
    let total_slots = cols * rows;
    let placeholder = if total_slots > count {
        // Derive dimensions from the first captured frame via ffprobe, falling back to 1280x720.
        let placeholder_path = tmp_dir.join("placeholder.jpg");
        let probe = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
            ])
            .arg(&successful[0])
            .output()
            .await;

        let (w, h) = probe
            .ok()
            .and_then(|out| {
                let s = String::from_utf8_lossy(&out.stdout);
                let mut parts = s.trim().splitn(2, ',');
                let w: usize = parts.next()?.parse().ok()?;
                let h: usize = parts.next()?.parse().ok()?;
                Some((w, h))
            })
            .unwrap_or((1280, 720));

        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("color=black:size={w}x{h}:rate=1"),
                "-frames:v",
                "1",
                "-y",
            ])
            .arg(&placeholder_path)
            .status()
            .await
            .context("failed to run ffmpeg — is it installed?")?;

        if !status.success() {
            bail!("ffmpeg failed to create placeholder image");
        }
        Some(placeholder_path)
    } else {
        None
    };

    // Build the full slot list, padding with the placeholder where needed.
    let mut slots: Vec<PathBuf> = successful.clone();
    for _ in count..total_slots {
        slots.push(
            placeholder
                .clone()
                .expect("placeholder must exist when slots exceed captures"),
        );
    }

    // Build the ffmpeg filter_complex expression.
    // Single image: no filter needed. 1 row: just hstack. 1 col: just vstack.
    // General case: hstack each row, then vstack all rows.
    let filter = if total_slots == 1 {
        // Single image — just copy it
        "[0]copy".to_string()
    } else if rows == 1 {
        // Single row — just hstack
        let inputs: String = (0..cols)
            .map(|c| format!("[{}]", c))
            .collect::<Vec<_>>()
            .join("");
        format!("{}hstack=inputs={}", inputs, cols)
    } else if cols == 1 {
        // Single column — just vstack
        let inputs: String = (0..rows)
            .map(|r| format!("[{}]", r))
            .collect::<Vec<_>>()
            .join("");
        format!("{}vstack=inputs={}", inputs, rows)
    } else {
        let mut f = String::new();
        for r in 0..rows {
            let row_inputs: String = (0..cols)
                .map(|c| format!("[{}]", r * cols + c))
                .collect::<Vec<_>>()
                .join("");
            f.push_str(&format!("{}hstack=inputs={}[row{}];", row_inputs, cols, r));
        }
        let row_labels: String = (0..rows)
            .map(|r| format!("[row{}]", r))
            .collect::<Vec<_>>()
            .join("");
        f.push_str(&format!("{}vstack=inputs={}", row_labels, rows));
        f
    };

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let output_path = {
        let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating output directory: {}", dir.display()))?;
        dir.join(format!("grid_{}.jpg", ts))
    };

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-loglevel", "error"]);
    for slot in &slots {
        cmd.arg("-i").arg(slot);
    }
    cmd.args([
        "-filter_complex",
        &filter,
        "-update",
        "1",
        "-frames:v",
        "1",
        "-y",
    ]);
    cmd.arg(&output_path);

    let status = cmd
        .status()
        .await
        .context("failed to run ffmpeg — is it installed?")?;

    let _ = std::fs::remove_dir_all(&tmp_dir);

    if !status.success() {
        bail!("ffmpeg exited with status {}", status);
    }

    if label {
        let display_ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        apply_label(&output_path, "grid", &display_ts).await?;
    }

    println!("Saved grid snapshot to {}", output_path.display());
    if failures > 0 {
        eprintln!("Warning: {} camera(s) failed to capture", failures);
    }

    Ok(())
}

async fn cmd_stream(
    config: &config::Config,
    name: &str,
    quality: StreamQuality,
    output: Option<PathBuf>,
    duration: u64,
    json: bool,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let url = cam.rtsp_url(quality);

    match output {
        Some(path) => {
            record_rtsp(&url, &path, duration).await?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "camera": name,
                        "file": path.display().to_string(),
                    })
                );
            } else {
                println!("Saved stream to {}", path.display());
            }
        }
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "camera": name,
                        "url": url,
                    })
                );
            } else {
                println!("{}", url);
            }
        }
    }
    Ok(())
}

async fn cmd_record(
    config: &config::Config,
    name: &str,
    output: Option<PathBuf>,
    duration: u64,
    json: bool,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let url = cam.rtsp_url(StreamQuality::Main);

    let path = output.unwrap_or_else(|| {
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("{}_{}.mp4", name, ts))
    });

    tracing::info!("recording {}s from '{}'...", duration, name);
    record_rtsp(&url, &path, duration).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "camera": name,
                "file": path.display().to_string(),
                "duration_secs": duration,
            })
        );
    } else {
        println!("Saved recording to {}", path.display());
    }
    Ok(())
}

async fn record_rtsp(url: &str, output: &PathBuf, duration: u64) -> Result<()> {
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-rtsp_transport",
            "tcp",
            "-i",
            url,
            "-t",
            &duration.to_string(),
            "-c",
            "copy",
            "-y",
        ])
        .arg(output)
        .status()
        .await
        .context("failed to run ffmpeg — is it installed?")?;

    if !status.success() {
        bail!("ffmpeg exited with status {}", status);
    }
    Ok(())
}

/// Watch mode: capture a snapshot every `interval` indefinitely until Ctrl+C.
async fn cmd_snapshot_watch(
    config: &config::Config,
    name: &str,
    output_dir: PathBuf,
    interval: Duration,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("creating output directory: {}", output_dir.display()))?;

    println!(
        "Watching '{}' — capturing every {} — saving to {} — Ctrl+C to stop",
        name,
        humantime::format_duration(interval),
        output_dir.display()
    );

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match cam.snapshot().await {
                    Ok(snapshot) => {
                        let ts = snapshot.timestamp.format("%Y%m%d_%H%M%S");
                        let filename = format!(
                            "{}_{}.{}",
                            snapshot.camera_name,
                            ts,
                            snapshot.format.extension()
                        );
                        let path = output_dir.join(&filename);
                        match std::fs::write(&path, &snapshot.data) {
                            Ok(()) => println!("Saved {}", path.display()),
                            Err(e) => eprintln!("Warning: failed to write {}: {}", path.display(), e),
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: snapshot failed for '{}': {}", name, e);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\nStopped watch mode for '{}'.", name);
                break;
            }
        }
    }

    Ok(())
}

async fn cmd_timelapse(
    config: &config::Config,
    name: &str,
    interval: Duration,
    total_duration: Duration,
    output: PathBuf,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    let (frames_dir, keep_frames) = match output_dir {
        Some(ref dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating frames directory: {}", dir.display()))?;
            (dir.clone(), true)
        }
        None => {
            let tmp = std::env::temp_dir().join(format!(
                "ipcam-timelapse-{}",
                chrono::Utc::now().timestamp()
            ));
            std::fs::create_dir_all(&tmp)
                .with_context(|| format!("creating temp frames directory: {}", tmp.display()))?;
            (tmp, false)
        }
    };

    let total_frames = {
        let secs = total_duration.as_secs_f64();
        let step = interval.as_secs_f64();
        (secs / step).ceil() as u64
    };

    println!(
        "Timelapse '{}': {} frames every {} — total {} — output {}",
        name,
        total_frames,
        humantime::format_duration(interval),
        humantime::format_duration(total_duration),
        output.display(),
    );
    println!("Press Ctrl+C to stop early and stitch whatever was captured.");

    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupted_signal = interrupted.clone();

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        interrupted_signal.store(true, Ordering::SeqCst);
        eprintln!("\nInterrupted — will stitch captured frames...");
    });

    let mut captured: u64 = 0;
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let deadline = tokio::time::Instant::now() + total_duration;

    loop {
        if interrupted.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {}
            _ = tokio::time::sleep_until(deadline) => { break; }
        }

        if tokio::time::Instant::now() >= deadline || interrupted.load(Ordering::SeqCst) {
            break;
        }

        match cam.snapshot().await {
            Ok(snapshot) => {
                captured += 1;
                let frame_path = frames_dir.join(format!("frame_{:04}.jpg", captured));
                std::fs::write(&frame_path, &snapshot.data)
                    .with_context(|| format!("writing frame {}", captured))?;
                println!(
                    "[{}/{}] Captured snapshot ({})",
                    captured, total_frames, name
                );
            }
            Err(e) => {
                eprintln!("Warning: snapshot failed for '{}': {}", name, e);
            }
        }
    }

    if captured == 0 {
        bail!("no frames were captured — cannot create timelapse");
    }

    println!("Stitching {} frames into {}...", captured, output.display());
    stitch_timelapse(&frames_dir, &output).await?;
    println!("Timelapse saved to {}", output.display());

    if !keep_frames {
        let _ = std::fs::remove_dir_all(&frames_dir);
    }

    Ok(())
}

/// Run ffmpeg to stitch sequentially-numbered JPEG frames into an MP4 at 30 fps.
async fn stitch_timelapse(frames_dir: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let input_pattern = frames_dir.join("frame_%04d.jpg");

    let status = tokio::process::Command::new("ffmpeg")
        .args(["-framerate", "30", "-i"])
        .arg(&input_pattern)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-y"])
        .arg(output)
        .status()
        .await
        .context("failed to run ffmpeg — is it installed?")?;

    if !status.success() {
        bail!("ffmpeg exited with status {}", status);
    }
    Ok(())
}

async fn cmd_events(config: &config::Config, name: &str, watch: bool, json: bool) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    if watch {
        let mut last_detected: Option<bool> = None;
        loop {
            match cam.motion_status().await {
                Ok(status) => {
                    let changed = last_detected != Some(status.detected);
                    if changed {
                        print_motion_status(name, &status, json);
                        last_detected = Some(status.detected);
                    }
                }
                Err(e) => {
                    eprintln!("error polling {}: {}", name, e);
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    } else {
        let status = cam.motion_status().await?;
        print_motion_status(name, &status, json);
        Ok(())
    }
}

fn print_motion_status(name: &str, status: &MotionStatus, json: bool) {
    if json {
        let ts = status.timestamp.map(|t| t.to_rfc3339()).unwrap_or_default();
        println!(
            "{}",
            serde_json::json!({
                "camera": name,
                "motion_detected": status.detected,
                "timestamp": ts,
            })
        );
    } else {
        let ts = status
            .timestamp
            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let state = if status.detected { "MOTION" } else { "clear" };
        println!("[{}] {}: {}", ts, name, state);
    }
}

async fn cmd_discover(
    timeout: u64,
    json: bool,
    config: &config::Config,
    subnets: &[&str],
) -> Result<()> {
    let mut cameras =
        discovery::discover_cameras(Duration::from_secs(timeout), Some(config)).await?;

    for cidr in subnets {
        if !json {
            println!("Scanning subnet {}...", cidr);
        }
        let subnet_cameras =
            discovery::scan_subnet(cidr, Duration::from_secs(timeout), Some(config)).await?;
        let seen: std::collections::HashSet<String> =
            cameras.iter().map(|c| c.address.clone()).collect();
        for cam in subnet_cameras {
            if !seen.contains(&cam.address) {
                cameras.push(cam);
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&cameras)?);
        return Ok(());
    }

    if cameras.is_empty() {
        println!("No cameras found on the network.");
        return Ok(());
    }

    println!(
        "{}",
        style::bold(&format!(
            "{:<18} {:<20} {:<20} {}",
            "ADDRESS", "MANUFACTURER", "MODEL", "ONVIF URL"
        ))
    );
    println!("{}", style::dim(&"-".repeat(90)));

    for cam in &cameras {
        println!(
            "{:<18} {:<20} {:<20} {}",
            cam.address,
            cam.manufacturer.as_deref().unwrap_or("-"),
            cam.model.as_deref().unwrap_or("-"),
            cam.onvif_url,
        );
    }

    println!();
    println!("Found {} camera(s).", cameras.len());
    Ok(())
}

async fn cmd_ptz(
    config: &config::Config,
    name: &str,
    action: PtzAction,
    preset: Option<u32>,
    speed: u8,
    json: bool,
) -> Result<()> {
    let cam_config = config.require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    let normalized_speed = speed as f32 / 9.0;

    let action_str = match action {
        PtzAction::Left => {
            cam.ptz_move(PtzDirection::Left, normalized_speed).await?;
            "left"
        }
        PtzAction::Right => {
            cam.ptz_move(PtzDirection::Right, normalized_speed).await?;
            "right"
        }
        PtzAction::Up => {
            cam.ptz_move(PtzDirection::Up, normalized_speed).await?;
            "up"
        }
        PtzAction::Down => {
            cam.ptz_move(PtzDirection::Down, normalized_speed).await?;
            "down"
        }
        PtzAction::Stop => {
            cam.ptz_stop().await?;
            "stop"
        }
        PtzAction::Preset => {
            let num = preset
                .ok_or_else(|| anyhow::anyhow!("preset number is required for 'preset' action"))?;
            cam.ptz_goto_preset(num).await?;
            "preset"
        }
        PtzAction::ZoomIn => {
            cam.ptz_zoom(normalized_speed).await?;
            "zoom-in"
        }
        PtzAction::ZoomOut => {
            cam.ptz_zoom(-normalized_speed).await?;
            "zoom-out"
        }
        PtzAction::Home => {
            cam.ptz_home().await?;
            "home"
        }
    };

    if json {
        let mut result = serde_json::json!({
            "camera": name,
            "action": action_str,
            "speed": speed,
            "success": true,
        });
        if let Some(p) = preset {
            result["preset"] = serde_json::json!(p);
        }
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        match action {
            PtzAction::Stop => println!("Stopped PTZ movement on '{}'", name),
            PtzAction::Preset => println!("Moving '{}' to preset {}", name, preset.unwrap()),
            PtzAction::Home => println!("Moving '{}' to home position", name),
            _ => println!("PTZ '{}' {} (speed {})", name, action_str, speed),
        }
    }

    Ok(())
}

async fn cmd_status(
    config: &config::Config,
    camera: Option<&str>,
    json: bool,
    limit: Option<u64>,
    offset: Option<u64>,
    fields: Option<&str>,
) -> Result<()> {
    let cameras_to_check: Vec<&config::CameraConfig> = match camera {
        Some(name) => {
            let cam = config.require_camera(name)?;
            vec![cam]
        }
        None => config.cameras.iter().collect(),
    };

    if cameras_to_check.is_empty() {
        if json {
            println!("{}", serde_json::json!({"items": [], "total": 0}));
        } else {
            println!(
                "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
            );
        }
        return Ok(());
    }

    let futures: Vec<_> = cameras_to_check
        .iter()
        .map(|cam_config| {
            let name = cam_config.name.clone();
            let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref());
            async move {
                match cam {
                    Ok(c) => {
                        let status = c.is_reachable().await;
                        (name, status)
                    }
                    Err(e) => (
                        name,
                        camera::HealthStatus {
                            online: false,
                            detail: e.to_string(),
                            latency: std::time::Duration::ZERO,
                        },
                    ),
                }
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    if json {
        let all_entries: Vec<serde_json::Value> = results
            .iter()
            .map(|(name, status)| {
                serde_json::json!({
                    "camera": name,
                    "online": status.online,
                    "detail": status.detail,
                    "latency_ms": status.latency.as_millis(),
                })
            })
            .collect();
        let total = all_entries.len();
        let offset_val = offset.unwrap_or(0) as usize;
        let entries: Vec<_> = all_entries.into_iter().skip(offset_val).collect();
        let entries: Vec<_> = if let Some(lim) = limit {
            entries.into_iter().take(lim as usize).collect()
        } else {
            entries
        };
        let entries = apply_fields_filter(entries, fields);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "items": entries,
                "total": total,
            }))?
        );
    } else {
        println!(
            "{}",
            style::bold(&format!(
                "{:<20} {:<10} {:<30} {}",
                "CAMERA", "STATUS", "MODEL", "LATENCY"
            ))
        );
        println!("{}", style::dim(&"-".repeat(70)));
        for (name, status) in &results {
            let state_raw = if status.online { "online" } else { "offline" };
            let state = if status.online {
                style::green(state_raw)
            } else {
                style::red(state_raw)
            };
            let latency_raw = format!("{}ms", status.latency.as_millis());
            // Pad columns manually to avoid ANSI codes breaking alignment
            let pad_state = 10_usize.saturating_sub(state_raw.len());
            let pad_detail = 30_usize.saturating_sub(status.detail.len());
            println!(
                "{:<20} {}{:pad_s$} {}{:pad_d$} {}",
                name,
                state,
                "",
                status.detail,
                "",
                style::dim(&latency_raw),
                pad_s = pad_state,
                pad_d = pad_detail,
            );
        }
    }

    Ok(())
}

async fn poll_all_cameras(config: &config::Config) -> Vec<(String, String, camera::HealthStatus)> {
    let futures: Vec<_> = config
        .cameras
        .iter()
        .map(|cam_config| {
            let name = cam_config.name.clone();
            let host = cam_config.host.clone();
            let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref());
            async move {
                match cam {
                    Ok(c) => {
                        let status = c.is_reachable().await;
                        (name, host, status)
                    }
                    Err(e) => (
                        name,
                        host,
                        camera::HealthStatus {
                            online: false,
                            detail: e.to_string(),
                            latency: std::time::Duration::ZERO,
                        },
                    ),
                }
            }
        })
        .collect();

    futures::future::join_all(futures).await
}

async fn cmd_watch(config: &config::Config, interval: Duration, exec: Option<&str>) -> Result<()> {
    if config.cameras.is_empty() {
        println!(
            "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
        );
        return Ok(());
    }

    // Map from camera name to last known online state.
    let mut last_state: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    // Initial poll — print all current states as the startup banner.
    let now = chrono::Local::now();
    let ts = now.format("%Y-%m-%d %H:%M:%S");
    println!(
        "[{}] Monitoring {} camera{} every {} (Ctrl+C to stop)",
        ts,
        config.cameras.len(),
        if config.cameras.len() == 1 { "" } else { "s" },
        humantime::format_duration(interval),
    );

    let initial = poll_all_cameras(config).await;
    for (name, _host, status) in &initial {
        let now = chrono::Local::now();
        let ts = now.format("%Y-%m-%d %H:%M:%S");
        let state = if status.online {
            style::green("online")
        } else {
            style::red("offline")
        };
        println!(
            "{} {}: {} ({}, {}ms)",
            style::dim(&format!("[{ts}]")),
            name,
            state,
            status.detail,
            status.latency.as_millis(),
        );
        last_state.insert(name.clone(), status.online);
    }

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the immediate first tick so the next fires after one full interval.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let results = poll_all_cameras(config).await;
                for (name, host, status) in &results {
                    let prev = last_state.get(name.as_str()).copied();
                    let changed = prev != Some(status.online);
                    if changed {
                        let now = chrono::Local::now();
                        let ts = now.format("%Y-%m-%d %H:%M:%S");
                        let state = if status.online {
                            style::green("online")
                        } else {
                            style::red("offline")
                        };
                        let annotation = match prev {
                            Some(false) => " ← back online",
                            Some(true)  => " ← went offline",
                            None        => "",
                        };
                        println!(
                            "{} {}: {}{} ({}, {}ms)",
                            style::dim(&format!("[{ts}]")),
                            name,
                            state,
                            annotation,
                            status.detail,
                            status.latency.as_millis(),
                        );
                        last_state.insert(name.clone(), status.online);

                        if let Some(cmd) = exec {
                            let status_str = if status.online { "online" } else { "offline" };
                            let run_result = tokio::process::Command::new("sh")
                                .arg("-c")
                                .arg(cmd)
                                .env("CAMERA_NAME", name)
                                .env("CAMERA_HOST", host)
                                .env("CAMERA_STATUS", status_str)
                                .env("CAMERA_DETAIL", &status.detail)
                                .status()
                                .await;
                            if let Err(e) = run_result {
                                eprintln!(
                                    "Warning: --exec command failed for '{}': {}",
                                    name, e
                                );
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\nStopped monitoring.");
                break;
            }
        }
    }

    Ok(())
}

fn cmd_config_path(json: bool) -> Result<()> {
    let path = config::Config::config_path()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path.display().to_string(),
                "exists": path.exists(),
            })
        );
    } else {
        println!("Config path: {}", path.display());
        if path.exists() {
            println!("Status: exists");
        } else {
            println!("Status: not found");
            println!();
            println!("Create it with:");
            println!();
            println!("  mkdir -p {}", path.parent().unwrap().display());
            println!("  cat > {} << 'EOF'", path.display());
            println!("[[cameras]]");
            println!("name = \"front-door\"");
            println!("type = \"reolink\"");
            println!("host = \"192.168.1.100\"");
            println!("username = \"admin\"");
            println!("password = \"your-password\"");
            println!("EOF");
        }
    }
    Ok(())
}

fn cmd_config_edit() -> Result<()> {
    let path = config::Config::config_path()?;

    // Ensure the config directory exists before opening the editor.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("launching editor '{editor}'"))?;

    if !status.success() {
        anyhow::bail!("editor '{editor}' exited with status {status}");
    }
    Ok(())
}

async fn cmd_config_check(config: &config::Config, json: bool) -> Result<()> {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    if config.cameras.is_empty() {
        errors.push("no cameras configured".to_string());
    }

    // Check for duplicate camera names
    let mut seen_names = std::collections::HashSet::new();
    for cam in &config.cameras {
        if !seen_names.insert(&cam.name) {
            errors.push(format!("duplicate camera name: '{}'", cam.name));
        }
    }

    // Check for duplicate hosts
    let mut seen_hosts: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for cam in &config.cameras {
        if let Some(first_name) = seen_hosts.get(cam.host.as_str()) {
            warnings.push(format!(
                "duplicate host '{}' (cameras '{}' and '{}')",
                cam.host, first_name, cam.name
            ));
        } else {
            seen_hosts.insert(&cam.host, &cam.name);
        }
    }

    // Per-camera checks
    for cam in &config.cameras {
        if cam.username.is_none() || cam.username.as_deref() == Some("") {
            warnings.push(format!("camera '{}': no username set", cam.name));
        }
        if cam.password.is_none() || cam.password.as_deref() == Some("") {
            warnings.push(format!("camera '{}': no password set", cam.name));
        }
        if cam.rtsp_port == 0 {
            errors.push(format!("camera '{}': invalid RTSP port 0", cam.name));
        }
        if cam.onvif_port == Some(0) {
            errors.push(format!("camera '{}': invalid ONVIF port 0", cam.name));
        }
        if cam.host.is_empty() {
            errors.push(format!("camera '{}': empty host", cam.name));
        }
    }

    // Check go2rtc references
    if config.go2rtc.is_none() {
        let has_go2rtc_stream = config.cameras.iter().any(|c| c.go2rtc_stream.is_some());
        if has_go2rtc_stream {
            errors.push(
                "cameras reference go2rtc_stream but no [go2rtc] section is configured".to_string(),
            );
        }
    }

    if json {
        let valid = errors.is_empty();
        println!(
            "{}",
            serde_json::json!({
                "valid": valid,
                "errors": errors,
                "warnings": warnings,
                "cameras": config.cameras.len(),
            })
        );
        if !valid {
            std::process::exit(1);
        }
    } else {
        let path = config::Config::config_path()?;
        println!("{}", style::bold(&format!("Config: {}", path.display())));
        println!("Cameras: {}", config.cameras.len());
        println!();

        if errors.is_empty() && warnings.is_empty() {
            println!("{}", style::green("No issues found."));
        }

        for e in &errors {
            println!("  {} {}", style::red("error:"), e);
        }
        for w in &warnings {
            println!("  {} {}", style::bold("warning:"), w);
        }

        if !errors.is_empty() {
            println!();
            println!(
                "{} error(s), {} warning(s)",
                style::red(&errors.len().to_string()),
                warnings.len()
            );
            std::process::exit(1);
        } else if !warnings.is_empty() {
            println!();
            println!("0 errors, {} warning(s)", warnings.len());
        }
    }

    Ok(())
}

fn cmd_config_show(json: bool, config_path: Option<&Path>) -> Result<()> {
    let mut cfg = config::Config::load(config_path)?;

    // Mask passwords in all camera entries.
    for cam in &mut cfg.cameras {
        if cam.password.is_some() {
            cam.password = Some("****".to_string());
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
    } else {
        let toml = toml::to_string_pretty(&cfg).context("serializing config to TOML")?;
        print!("{toml}");
    }
    Ok(())
}

/// Result of a single test step.
struct StepResult {
    passed: bool,
    elapsed: Duration,
    /// Failure message; empty when passed.
    message: String,
}

/// Run all three test steps for one camera and return per-step results.
async fn test_camera(
    cam_config: &config::CameraConfig,
    go2rtc: Option<&config::Go2rtcConfig>,
) -> [StepResult; 3] {
    // --- Step 1: TCP reachability ---
    let reachable = {
        let start = std::time::Instant::now();
        let addr = format!("{}:{}", cam_config.host, cam_config.rtsp_port);
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(&addr),
        )
        .await;
        let elapsed = start.elapsed();
        match outcome {
            Ok(Ok(_)) => StepResult {
                passed: true,
                elapsed,
                message: String::new(),
            },
            Ok(Err(e)) => StepResult {
                passed: false,
                elapsed,
                message: e.to_string(),
            },
            Err(_) => StepResult {
                passed: false,
                elapsed,
                message: "connection timed out".to_string(),
            },
        }
    };

    // --- Step 2: RTSP stream probe via ffprobe ---
    let rtsp = if !reachable.passed {
        StepResult {
            passed: false,
            elapsed: Duration::ZERO,
            message: "skipped".to_string(),
        }
    } else {
        let start = std::time::Instant::now();
        let cam = vendors::create_camera(cam_config, go2rtc);
        match cam {
            Err(e) => StepResult {
                passed: false,
                elapsed: start.elapsed(),
                message: e.to_string(),
            },
            Ok(c) => {
                let url = c.rtsp_url(StreamQuality::Main);
                let probe = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::process::Command::new("ffprobe")
                        .args(["-v", "quiet", "-rtsp_transport", "tcp", "-i", &url])
                        .output(),
                )
                .await;
                let elapsed = start.elapsed();
                match probe {
                    Ok(Ok(out)) if out.status.success() => StepResult {
                        passed: true,
                        elapsed,
                        message: String::new(),
                    },
                    Ok(Ok(out)) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let detail = stderr
                            .lines()
                            .next()
                            .unwrap_or("ffprobe failed")
                            .trim()
                            .to_string();
                        StepResult {
                            passed: false,
                            elapsed,
                            message: detail,
                        }
                    }
                    Ok(Err(e)) => StepResult {
                        passed: false,
                        elapsed,
                        message: format!("ffprobe error: {e}"),
                    },
                    Err(_) => StepResult {
                        passed: false,
                        elapsed,
                        message: "timed out after 5s".to_string(),
                    },
                }
            }
        }
    };

    // --- Step 3: Snapshot ---
    let snapshot = if !reachable.passed {
        StepResult {
            passed: false,
            elapsed: Duration::ZERO,
            message: "skipped".to_string(),
        }
    } else {
        let start = std::time::Instant::now();
        let cam = vendors::create_camera(cam_config, go2rtc);
        match cam {
            Err(e) => StepResult {
                passed: false,
                elapsed: start.elapsed(),
                message: e.to_string(),
            },
            Ok(c) => {
                let result = tokio::time::timeout(Duration::from_secs(15), c.snapshot()).await;
                let elapsed = start.elapsed();
                match result {
                    Ok(Ok(_)) => StepResult {
                        passed: true,
                        elapsed,
                        message: String::new(),
                    },
                    Ok(Err(e)) => StepResult {
                        passed: false,
                        elapsed,
                        message: e.to_string(),
                    },
                    Err(_) => StepResult {
                        passed: false,
                        elapsed,
                        message: "timed out after 15s".to_string(),
                    },
                }
            }
        }
    };

    [reachable, rtsp, snapshot]
}

/// Format elapsed duration for display: sub-second as ms, otherwise as fractional seconds.
fn fmt_elapsed(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

async fn cmd_test(config: &config::Config, camera: Option<&str>, json: bool) -> Result<()> {
    let cameras_to_test: Vec<&config::CameraConfig> = match camera {
        Some(name) => {
            let cam = config.require_camera(name)?;
            vec![cam]
        }
        None => config.cameras.iter().collect(),
    };

    if cameras_to_test.is_empty() {
        if json {
            println!("[]");
        } else {
            println!(
                "No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually."
            );
        }
        return Ok(());
    }

    let go2rtc = config.go2rtc.clone();

    let futures: Vec<_> = cameras_to_test
        .iter()
        .map(|cam_config| {
            let name = cam_config.name.clone();
            let cam_type = cam_config.camera_type.to_string();
            let host = cam_config.host.clone();
            let go2rtc_ref = go2rtc.as_ref();
            let cam_config = *cam_config;
            async move {
                let results = test_camera(cam_config, go2rtc_ref).await;
                (name, cam_type, host, results)
            }
        })
        .collect();

    let all_results = futures::future::join_all(futures).await;

    if json {
        let entries: Vec<serde_json::Value> = all_results
            .iter()
            .map(|(name, cam_type, host, steps)| {
                let [ref reachable, ref rtsp, ref snapshot] = *steps;
                serde_json::json!({
                    "camera": name,
                    "type": cam_type,
                    "host": host,
                    "steps": {
                        "reachable": {
                            "passed": reachable.passed,
                            "elapsed_ms": reachable.elapsed.as_millis(),
                            "message": reachable.message,
                        },
                        "rtsp_stream": {
                            "passed": rtsp.passed,
                            "elapsed_ms": rtsp.elapsed.as_millis(),
                            "message": rtsp.message,
                        },
                        "snapshot": {
                            "passed": snapshot.passed,
                            "elapsed_ms": snapshot.elapsed.as_millis(),
                            "message": snapshot.message,
                        },
                    }
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for (name, cam_type, host, steps) in &all_results {
            println!("Testing {name} ({cam_type} @ {host})...");
            let labels = ["reachable", "RTSP stream", "snapshot"];
            let [ref reachable, ref rtsp, ref snapshot] = *steps;
            for (label, step) in labels.iter().zip([reachable, rtsp, snapshot]) {
                let icon = if step.passed { "✓" } else { "✗" };
                let timing = fmt_elapsed(step.elapsed);
                if step.message.is_empty() {
                    println!("  {icon} {label:<18} ({timing})");
                } else {
                    println!("  {icon} {label:<18} ({timing}) — {}", step.message);
                }
            }
            println!();
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_add_direct(
    host: &str,
    name: Option<&str>,
    camera_type_arg: CameraTypeArg,
    username: &str,
    password: Option<&str>,
    rtsp_port: u16,
    go2rtc_stream: Option<&str>,
    config_path: Option<&Path>,
    json: bool,
) -> Result<()> {
    let mut config = config::Config::load(config_path)?;

    // Auto-generate name from last octet if not provided.
    let resolved_name = match name {
        Some(n) => n.to_string(),
        None => {
            let last_octet = host
                .rsplit('.')
                .next()
                .unwrap_or(host)
                .split(':')
                .next()
                .unwrap_or(host);
            format!("camera-{}", last_octet)
        }
    };

    if config.cameras.iter().any(|c| c.name == resolved_name) {
        bail!(
            "a camera named '{}' already exists in config",
            resolved_name
        );
    }

    if config.cameras.iter().any(|c| c.host == host) {
        bail!("a camera with host '{}' already exists in config", host);
    }

    let camera_type = config::CameraType::from(camera_type_arg);

    let new_camera = config::CameraConfig {
        name: resolved_name.clone(),
        camera_type,
        host: host.to_string(),
        rtsp_port,
        username: if username.is_empty() {
            None
        } else {
            Some(username.to_string())
        },
        password: password.map(|p| p.to_string()),
        go2rtc_stream: go2rtc_stream.map(|s| s.to_string()),
        onvif_port: None,
        main_stream: None,
        sub_stream: None,
        onvif_username: None,
        onvif_password: None,
    };

    config.cameras.push(new_camera);

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "added",
                "camera": resolved_name,
                "host": host,
                "type": camera_type.to_string(),
                "rtsp_port": rtsp_port,
            })
        );
    } else {
        println!(
            "Added camera '{}' ({} @ {})",
            resolved_name, camera_type, host
        );
    }

    Ok(())
}

async fn cmd_add_discover(
    config: &config::Config,
    config_path: Option<&Path>,
    json: bool,
    timeout: u64,
    subnets: &[&str],
) -> Result<()> {
    use crate::discovery::{discover_cameras, scan_subnet};
    use crate::init::{auto_camera_config, infer_camera_type, prompt_for_camera};

    let existing_hosts: std::collections::HashSet<&str> =
        config.cameras.iter().map(|c| c.host.as_str()).collect();

    if !json {
        println!("Scanning network for cameras...");
    }

    let mut discovered = discover_cameras(Duration::from_secs(timeout), Some(config)).await?;

    for cidr in subnets {
        if !json {
            println!("Scanning subnet {}...", cidr);
        }
        let subnet_cameras = scan_subnet(cidr, Duration::from_secs(timeout), Some(config)).await?;
        let seen: std::collections::HashSet<String> =
            discovered.iter().map(|c| c.address.clone()).collect();
        for cam in subnet_cameras {
            if !seen.contains(&cam.address) {
                discovered.push(cam);
            }
        }
    }

    // Filter out cameras already in config
    let new_cameras: Vec<_> = discovered
        .iter()
        .filter(|c| !existing_hosts.contains(c.address.as_str()))
        .collect();

    if new_cameras.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "discovered": discovered.len(),
                    "new": 0,
                    "added": [],
                    "skipped": [],
                })
            );
        } else {
            println!(
                "Found {} camera(s), all already configured.",
                discovered.len()
            );
        }
        return Ok(());
    }

    if !json {
        println!("Found {} new camera(s):\n", new_cameras.len());
        for (i, cam) in new_cameras.iter().enumerate() {
            let display = match (&cam.manufacturer, &cam.model) {
                (Some(mfr), Some(mdl)) => format!("{} ({})", mfr, mdl),
                (Some(mfr), None) => mfr.clone(),
                (None, Some(mdl)) => mdl.clone(),
                (None, None) => "Unknown".to_string(),
            };
            let type_hint = infer_camera_type(cam.manufacturer.as_deref())
                .map(|t| format!(" [{}]", t))
                .unwrap_or_default();
            println!("  {}. {} — {}{}", i + 1, cam.address, display, type_hint);
        }

        if !config.cameras.is_empty() {
            let names: Vec<&str> = config.cameras.iter().map(|c| c.name.as_str()).collect();
            println!("\nAlready configured: {}", names.join(", "));
        }
    }

    let mut added = Vec::new();
    let mut skipped = Vec::new();
    let mut cfg = config.clone();

    if json {
        // Auto mode: add all cameras we can infer a type for
        for cam in &new_cameras {
            match auto_camera_config(cam) {
                Some(c) => {
                    added.push(serde_json::json!({
                        "name": c.name,
                        "host": c.host,
                        "type": c.camera_type.to_string(),
                    }));
                    cfg.cameras.push(c);
                }
                None => {
                    skipped.push(serde_json::json!({
                        "host": cam.address,
                        "reason": "could not infer camera type",
                        "manufacturer": cam.manufacturer,
                    }));
                }
            }
        }
    } else {
        // Interactive mode: prompt for each camera
        for cam in &new_cameras {
            if let Some(c) = prompt_for_camera(cam)? {
                added.push(serde_json::json!({
                    "name": c.name,
                    "host": c.host,
                    "type": c.camera_type.to_string(),
                }));
                cfg.cameras.push(c);
            }
        }
    }

    if !added.is_empty() {
        let path = config_path
            .map(|p| p.to_path_buf())
            .unwrap_or(config::Config::config_path()?);
        let content = toml::to_string_pretty(&cfg).context("serializing config")?;
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "discovered": discovered.len(),
                "new": new_cameras.len(),
                "added": added,
                "skipped": skipped,
            })
        );
    } else if added.is_empty() {
        println!("\nNo cameras added.");
    } else {
        println!("\nAdded {} camera(s).", added.len());
    }

    Ok(())
}

fn cmd_remove(name: &str, yes: bool, config_path: Option<&Path>, json: bool) -> Result<()> {
    let mut config = config::Config::load(config_path)?;

    // Validate the camera exists (require_camera gives a good error message)
    config.require_camera(name)?;
    let pos = config
        .cameras
        .iter()
        .position(|c| c.name == name)
        .expect("require_camera succeeded so position must exist");

    let cam = &config.cameras[pos];
    let cam_type = cam.camera_type.to_string();
    let cam_host = cam.host.clone();

    if std::io::stdout().is_terminal() {
        println!(
            "Will remove: '{}' ({} @ {})",
            cam.name, cam.camera_type, cam.host
        );
    }

    if !yes {
        if !std::io::stdout().is_terminal() {
            let envelope = serde_json::json!({
                "error": {
                    "kind": "confirmation_required",
                    "message": format!("Removing camera '{}' requires confirmation", name),
                    "hint": "Re-run with --yes to confirm."
                }
            });
            eprintln!("{}", envelope);
            std::process::exit(2);
        } else {
            use std::io::Write as _;
            print!("Confirm removal? [y/N]: ");
            std::io::stdout().flush().context("flush stdout")?;
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .context("read from stdin")?;
            let answer = line.trim().to_lowercase();
            if answer != "y" && answer != "yes" {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    config.cameras.remove(pos);

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "removed",
                "camera": name,
                "host": cam_host,
                "type": cam_type,
            })
        );
    } else {
        println!("Removed camera '{}'.", name);
    }

    Ok(())
}

fn cmd_rename(
    old_name: &str,
    new_name: &str,
    config_path: Option<&Path>,
    json: bool,
) -> Result<()> {
    let mut config = config::Config::load(config_path)?;

    // Verify old_name exists (require_camera gives a good error message).
    config.require_camera(old_name)?;
    let pos = config
        .cameras
        .iter()
        .position(|c| c.name == old_name)
        .expect("require_camera succeeded so position must exist");

    // Verify new_name is not already taken.
    if config.cameras.iter().any(|c| c.name == new_name) {
        bail!("a camera named '{}' already exists in config", new_name);
    }

    // Auto-generated pattern: name with hyphens replaced by underscores.
    let old_auto = old_name.replace('-', "_");
    let new_auto = new_name.replace('-', "_");

    let cam = &mut config.cameras[pos];
    cam.name = new_name.to_string();

    let mut updated_go2rtc = false;
    if let Some(ref stream) = cam.go2rtc_stream.clone()
        && *stream == old_auto
    {
        cam.go2rtc_stream = Some(new_auto.clone());
        updated_go2rtc = true;
    }

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "renamed",
                "old_name": old_name,
                "new_name": new_name,
                "updated_go2rtc": updated_go2rtc,
            })
        );
    } else {
        println!("Renamed camera '{}' -> '{}'", old_name, new_name);
        if updated_go2rtc {
            println!("  go2rtc_stream: '{}' -> '{}'", old_auto, new_auto);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_seconds() {
        let d = parse_duration("30s").unwrap();
        assert_eq!(d, Duration::from_secs(30));
    }

    #[test]
    fn parse_duration_minutes() {
        let d = parse_duration("5m").unwrap();
        assert_eq!(d, Duration::from_secs(300));
    }

    #[test]
    fn parse_duration_hours() {
        let d = parse_duration("1h").unwrap();
        assert_eq!(d, Duration::from_secs(3600));
    }

    #[test]
    fn parse_duration_compound() {
        let d = parse_duration("2h30m").unwrap();
        assert_eq!(d, Duration::from_secs(2 * 3600 + 30 * 60));
    }

    #[test]
    fn parse_duration_invalid() {
        assert!(parse_duration("not-a-duration").is_err());
    }

    #[test]
    fn parse_duration_error_message_contains_input() {
        let err = parse_duration("xyz").unwrap_err();
        assert!(err.to_string().contains("xyz"));
    }

    #[test]
    fn redact_url_with_credentials() {
        let url = "rtsp://admin:secret@192.168.1.100:554/stream";
        assert_eq!(redact_url(url), "rtsp://****:****@192.168.1.100:554/stream");
    }

    #[test]
    fn redact_url_without_credentials() {
        let url = "rtsp://192.168.1.100:554/stream";
        assert_eq!(redact_url(url), "rtsp://192.168.1.100:554/stream");
    }

    #[test]
    fn redact_url_https() {
        let url = "https://user:pass@example.com/api";
        assert_eq!(redact_url(url), "https://****:****@example.com/api");
    }

    #[test]
    fn redact_url_no_scheme() {
        let url = "just-a-string";
        assert_eq!(redact_url(url), "just-a-string");
    }
}
