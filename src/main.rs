mod camera;
mod config;
mod discovery;
mod frigate;
mod init;
mod vendors;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use camera::{MotionStatus, PtzDirection, StreamQuality};
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ipcam", about = "Manage IP cameras from the command line")]
struct Cli {
    /// Path to config file (default: ~/.config/ipcam/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List configured cameras
    List,

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
        #[arg(short, long, default_value = "main")]
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
        #[arg(short, long, default_value = "main")]
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

    /// Discover cameras on the local network via ONVIF WS-Discovery
    Discover {
        /// How long to wait for responses (seconds)
        #[arg(short, long, default_value = "3")]
        timeout: u64,
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
    },

    /// Interact with Frigate NVR
    Frigate {
        #[command(subcommand)]
        action: FrigateAction,
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

    /// Rename a camera in the config file
    Rename {
        /// Current camera name
        old_name: String,
        /// New camera name
        new_name: String,
    },

    /// Manually add a camera to the config
    Add {
        /// IP address of the camera
        host: String,

        /// Camera name (default: auto-generated from last octet, e.g. "camera-215")
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
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum PtzAction {
    Left,
    Right,
    Up,
    Down,
    Stop,
    Preset,
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
enum FrigateAction {
    /// List recent events from Frigate
    Events {
        /// Filter by camera name
        #[arg(short, long)]
        camera: Option<String>,

        /// Maximum number of events to return
        #[arg(short, long, default_value = "10")]
        limit: u32,
    },

    /// Save the latest snapshot from a Frigate camera
    Snapshot {
        /// Camera name (uses Frigate naming, e.g. front_door)
        camera: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config file path and status (default)
    Path,

    /// Open the config file in $EDITOR (or $VISUAL, fallback to vim)
    Edit,

    /// Print the current config with passwords masked
    Show,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ipcam=info".parse().unwrap()),
        )
        .init();

    if let Err(err) = run().await {
        print_error(&err);
        std::process::exit(1);
    }
}

fn print_error(err: &anyhow::Error) {
    eprintln!("Error: {err}");

    let mut source = err.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }

    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("connection refused")
        || msg.contains("timed out")
        || msg.contains("no route to host")
        || msg.contains("network unreachable")
    {
        eprintln!();
        eprintln!("Hint: Check that the camera is powered on and connected to your network.");
    } else if msg.contains("ffmpeg")
        && (msg.contains("no such file or directory") || msg.contains("os error 2"))
    {
        eprintln!();
        eprintln!("Hint: ffmpeg is not installed. Install it with:");
        eprintln!("  brew install ffmpeg    (macOS)");
        eprintln!("  apt install ffmpeg     (Debian/Ubuntu)");
        eprintln!("  dnf install ffmpeg     (Fedora)");
    } else if msg.contains("401")
        || msg.contains("403")
        || msg.contains("unauthorized")
        || msg.contains("authentication")
        || msg.contains("wrong password")
    {
        eprintln!();
        eprintln!("Hint: Check the username and password in your config file.");
    }

    if std::env::var("RUST_BACKTRACE").as_deref() == Ok("1")
        || std::env::var("RUST_BACKTRACE").as_deref() == Ok("full")
    {
        let bt = err.backtrace();
        let bt_str = bt.to_string();
        if !bt_str.is_empty() && bt_str != "disabled backtrace" {
            eprintln!();
            eprintln!("{bt_str}");
        }
    }
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

async fn run() -> Result<()> {
    let cli = Cli::parse();

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

    config::Config::migrate_if_needed()?;

    // When no config file exists, give a helpful message for commands that need cameras.
    let needs_cameras = !matches!(
        cli.command,
        Command::Config { .. } | Command::Discover { .. }
    );
    if needs_cameras && !config::Config::config_exists()? {
        let path = config::Config::config_path()?;
        bail!(
            "no cameras configured. Run `ipcam init` to set up your cameras, \
             or create a config at {}",
            path.display()
        );
    }

    let config = config::Config::load()?;

    match cli.command {
        Command::List => cmd_list(&config, cli.json),
        Command::Info { camera } => cmd_info(&config, &camera, cli.json).await,
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
                cmd_snapshot_all(&config, output_dir, cli.json, label).await
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
                cmd_snapshot(&config, &name, output, label, preview).await
            }
        }
        Command::Live {
            camera,
            quality,
            window,
        } => cmd_live(&config, &camera, quality, window).await,
        Command::Preview { camera, sub: _ } => cmd_preview(&config, &camera).await,
        Command::SnapshotAll { output_dir } => {
            cmd_snapshot_all(&config, output_dir, cli.json, false).await
        }
        Command::Stream {
            camera,
            quality,
            output,
            duration,
        } => cmd_stream(&config, &camera, quality, output, duration).await,
        Command::Record {
            camera,
            output,
            duration,
        } => cmd_record(&config, &camera, output, duration).await,
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
            ConfigAction::Path => cmd_config_path(),
            ConfigAction::Edit => cmd_config_edit(),
            ConfigAction::Show => cmd_config_show(),
        },
        Command::Discover { timeout } => cmd_discover(timeout, cli.json).await,
        Command::Events { camera, watch } => cmd_events(&config, &camera, watch, cli.json).await,
        Command::Status { camera } => cmd_status(&config, camera.as_deref(), cli.json).await,
        Command::Frigate { action } => cmd_frigate(&config, action, cli.json).await,
        Command::Ptz {
            camera,
            action,
            preset,
            speed,
        } => cmd_ptz(&config, &camera, action, preset, speed).await,
        Command::Watch { interval, exec } => {
            let interval = parse_duration(&interval)?;
            cmd_watch(&config, interval, exec.as_deref()).await
        }
        Command::Test { camera } => cmd_test(&config, camera.as_deref(), cli.json).await,
        Command::Rename { old_name, new_name } => cmd_rename(&old_name, &new_name),
        Command::Add {
            host,
            name,
            r#type,
            username,
            password,
            rtsp_port,
            go2rtc_stream,
        } => cmd_add(&host, name.as_deref(), r#type, &username, password.as_deref(), rtsp_port, go2rtc_stream.as_deref()),
        Command::Remove { name, yes } => cmd_remove(&name, yes),
        Command::Completions { .. } | Command::Init { .. } => {
            unreachable!("handled before config load")
        }
    }
}

fn cmd_list(config: &config::Config, json: bool) -> Result<()> {
    if config.cameras.is_empty() {
        if json {
            println!("[]");
        } else {
            let config_path = config::Config::config_path()?;
            println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
            println!("Config file: {}", config_path.display());
        }
        return Ok(());
    }

    if json {
        let cameras: Vec<_> = config
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
        println!("{}", serde_json::to_string_pretty(&cameras)?);
    } else {
        for cam in &config.cameras {
            println!("{:<20} {:<10} {}", cam.name, cam.camera_type, cam.host);
        }
    }
    Ok(())
}

async fn cmd_info(config: &config::Config, name: &str, json: bool) -> Result<()> {
    let cam_config = config
        .require_camera(name)?;
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
) -> Result<()> {
    let cam_config = config
        .require_camera(name)?;
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

    println!("Saved snapshot to {}", path.display());

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
    let cam_config = config
        .require_camera(name)?;
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
        println!("Opening live stream from '{}' in ffplay...", name);
        let status = tokio::process::Command::new("ffplay")
            .args([
                "-rtsp_transport", "tcp",
                "-loglevel", "error",
                "-window_title", &format!("ipcam - {}", name),
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
    println!("Live view of '{}' (press Ctrl+C to stop)...", name);
    let temp_path = std::env::temp_dir().join(format!("ipcam_live_{}.jpg", name));

    loop {
        let grab = async {
            let status = tokio::process::Command::new("ffmpeg")
                .args([
                    "-rtsp_transport", "tcp",
                    "-loglevel", "error",
                    "-y",
                    "-i", &url,
                    "-frames:v", "1",
                    "-update", "1",
                ])
                .arg(temp_path.as_os_str())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            if let Ok(s) = status && s.success() && temp_path.exists() {
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
            println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
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
                                    let display_ts = snapshot
                                        .timestamp
                                        .format("%Y-%m-%d %H:%M:%S")
                                        .to_string();
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
        println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
        return Ok(());
    }

    let n = config.cameras.len();

    // Capture all cameras in parallel into a temp directory.
    let tmp_dir = std::env::temp_dir().join(format!(
        "ipcam-grid-{}",
        chrono::Utc::now().timestamp()
    ));
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
                "-loglevel", "error",
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
        let inputs: String = (0..cols).map(|c| format!("[{}]", c)).collect::<Vec<_>>().join("");
        format!("{}hstack=inputs={}", inputs, cols)
    } else if cols == 1 {
        // Single column — just vstack
        let inputs: String = (0..rows).map(|r| format!("[{}]", r)).collect::<Vec<_>>().join("");
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
    cmd.args(["-filter_complex", &filter, "-update", "1", "-frames:v", "1", "-y"]);
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
) -> Result<()> {
    let cam_config = config
        .require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let url = cam.rtsp_url(quality);

    match output {
        Some(path) => {
            record_rtsp(&url, &path, duration).await?;
            println!("Saved stream to {}", path.display());
        }
        None => {
            println!("{}", url);
        }
    }
    Ok(())
}

async fn cmd_record(
    config: &config::Config,
    name: &str,
    output: Option<PathBuf>,
    duration: u64,
) -> Result<()> {
    let cam_config = config
        .require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;
    let url = cam.rtsp_url(StreamQuality::Main);

    let path = output.unwrap_or_else(|| {
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("{}_{}.mp4", name, ts))
    });

    tracing::info!("recording {}s from '{}'...", duration, name);
    record_rtsp(&url, &path, duration).await?;
    println!("Saved recording to {}", path.display());
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
    let cam_config = config
        .require_camera(name)?;
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
    let cam_config = config
        .require_camera(name)?;
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
    let cam_config = config
        .require_camera(name)?;
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

async fn cmd_discover(timeout: u64, json: bool) -> Result<()> {
    let cameras = discovery::discover_cameras(Duration::from_secs(timeout)).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&cameras)?);
        return Ok(());
    }

    if cameras.is_empty() {
        println!("No cameras found on the network.");
        return Ok(());
    }

    println!(
        "{:<18} {:<20} {:<20} ONVIF URL",
        "ADDRESS", "MANUFACTURER", "MODEL"
    );
    println!("{}", "-".repeat(90));

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
) -> Result<()> {
    let cam_config = config
        .require_camera(name)?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    // Normalize speed from 1-9 range to 0.0-1.0
    let normalized_speed = speed as f32 / 9.0;

    match action {
        PtzAction::Left => {
            cam.ptz_move(PtzDirection::Left, normalized_speed).await?;
            println!("Moving '{}' left (speed {})", name, speed);
        }
        PtzAction::Right => {
            cam.ptz_move(PtzDirection::Right, normalized_speed).await?;
            println!("Moving '{}' right (speed {})", name, speed);
        }
        PtzAction::Up => {
            cam.ptz_move(PtzDirection::Up, normalized_speed).await?;
            println!("Moving '{}' up (speed {})", name, speed);
        }
        PtzAction::Down => {
            cam.ptz_move(PtzDirection::Down, normalized_speed).await?;
            println!("Moving '{}' down (speed {})", name, speed);
        }
        PtzAction::Stop => {
            cam.ptz_stop().await?;
            println!("Stopped PTZ movement on '{}'", name);
        }
        PtzAction::Preset => {
            let num = preset
                .ok_or_else(|| anyhow::anyhow!("preset number is required for 'preset' action"))?;
            cam.ptz_goto_preset(num).await?;
            println!("Moving '{}' to preset {}", name, num);
        }
    }

    Ok(())
}

async fn cmd_status(config: &config::Config, camera: Option<&str>, json: bool) -> Result<()> {
    let cameras_to_check: Vec<&config::CameraConfig> = match camera {
        Some(name) => {
            let cam = config
                .require_camera(name)?;
            vec![cam]
        }
        None => config.cameras.iter().collect(),
    };

    if cameras_to_check.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
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
        let entries: Vec<_> = results
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
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for (name, status) in &results {
            let state = if status.online { "online" } else { "offline" };
            println!(
                "{:<20} {:<8} ({}, {}ms)",
                name,
                state,
                status.detail,
                status.latency.as_millis(),
            );
        }
    }

    Ok(())
}

async fn poll_all_cameras(
    config: &config::Config,
) -> Vec<(String, String, camera::HealthStatus)> {
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

async fn cmd_watch(
    config: &config::Config,
    interval: Duration,
    exec: Option<&str>,
) -> Result<()> {
    if config.cameras.is_empty() {
        println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
        return Ok(());
    }

    // Map from camera name to last known online state.
    let mut last_state: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();

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
        let state = if status.online { "online" } else { "offline" };
        println!(
            "[{}] {}: {} ({}, {}ms)",
            ts,
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
                        let state = if status.online { "online" } else { "offline" };
                        let annotation = match prev {
                            Some(false) => " ← back online",
                            Some(true)  => " ← went offline",
                            None        => "",
                        };
                        println!(
                            "[{}] {}: {}{} ({}, {}ms)",
                            ts,
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

async fn cmd_frigate(config: &config::Config, action: FrigateAction, json: bool) -> Result<()> {
    let frigate_config = config
        .frigate
        .as_ref()
        .context("no [frigate] section in config file")?;
    let client = frigate::FrigateClient::new(frigate_config);

    match action {
        FrigateAction::Events { camera, limit } => {
            // If user passes a ipcam name, resolve its frigate_name
            let frigate_camera = camera.as_ref().map(|name| {
                config
                    .find_camera(name)
                    .map(|c| c.frigate_name())
                    .unwrap_or_else(|| name.clone())
            });

            let events = client.events(frigate_camera.as_deref(), limit).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else {
                if events.is_empty() {
                    println!("No events found.");
                    return Ok(());
                }
                println!(
                    "{:<24} {:<18} {:<12} {:<8}",
                    "TIME", "CAMERA", "LABEL", "SCORE"
                );
                println!("{}", "-".repeat(64));
                for event in &events {
                    let ts = chrono::DateTime::from_timestamp(event.start_time as i64, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| format!("{:.0}", event.start_time));
                    let score = event
                        .score
                        .map(|s| format!("{:.0}%", s * 100.0))
                        .unwrap_or_else(|| "-".to_string());
                    println!(
                        "{:<24} {:<18} {:<12} {:<8}",
                        ts, event.camera, event.label, score,
                    );
                }
            }
        }
        FrigateAction::Snapshot { camera, output } => {
            // If user passes a ipcam name, resolve its frigate_name
            let frigate_camera = config
                .find_camera(&camera)
                .map(|c| c.frigate_name())
                .unwrap_or_else(|| camera.clone());

            let path = client.snapshot(&frigate_camera, output).await?;
            println!("Saved Frigate snapshot to {}", path.display());
        }
    }

    Ok(())
}

fn cmd_config_path() -> Result<()> {
    let path = config::Config::config_path()?;
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

fn cmd_config_show() -> Result<()> {
    let mut cfg = config::Config::load()?;

    // Mask passwords in all camera entries.
    for cam in &mut cfg.cameras {
        if cam.password.is_some() {
            cam.password = Some("****".to_string());
        }
    }

    let toml = toml::to_string_pretty(&cfg)
        .context("serializing config to TOML")?;
    print!("{toml}");
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
async fn test_camera(cam_config: &config::CameraConfig, go2rtc: Option<&config::Go2rtcConfig>) -> [StepResult; 3] {
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
            Ok(Ok(_)) => StepResult { passed: true, elapsed, message: String::new() },
            Ok(Err(e)) => StepResult { passed: false, elapsed, message: e.to_string() },
            Err(_) => StepResult {
                passed: false,
                elapsed,
                message: "connection timed out".to_string(),
            },
        }
    };

    // --- Step 2: RTSP stream probe via ffprobe ---
    let rtsp = if !reachable.passed {
        StepResult { passed: false, elapsed: Duration::ZERO, message: "skipped".to_string() }
    } else {
        let start = std::time::Instant::now();
        let cam = vendors::create_camera(cam_config, go2rtc);
        match cam {
            Err(e) => StepResult { passed: false, elapsed: start.elapsed(), message: e.to_string() },
            Ok(c) => {
                let url = c.rtsp_url(StreamQuality::Main);
                let probe = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::process::Command::new("ffprobe")
                        .args([
                            "-v", "quiet",
                            "-rtsp_transport", "tcp",
                            "-i", &url,
                        ])
                        .output(),
                )
                .await;
                let elapsed = start.elapsed();
                match probe {
                    Ok(Ok(out)) if out.status.success() => {
                        StepResult { passed: true, elapsed, message: String::new() }
                    }
                    Ok(Ok(out)) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let detail = stderr.lines().next().unwrap_or("ffprobe failed").trim().to_string();
                        StepResult { passed: false, elapsed, message: detail }
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
        StepResult { passed: false, elapsed: Duration::ZERO, message: "skipped".to_string() }
    } else {
        let start = std::time::Instant::now();
        let cam = vendors::create_camera(cam_config, go2rtc);
        match cam {
            Err(e) => StepResult { passed: false, elapsed: start.elapsed(), message: e.to_string() },
            Ok(c) => {
                let result = tokio::time::timeout(Duration::from_secs(15), c.snapshot()).await;
                let elapsed = start.elapsed();
                match result {
                    Ok(Ok(_)) => StepResult { passed: true, elapsed, message: String::new() },
                    Ok(Err(e)) => StepResult { passed: false, elapsed, message: e.to_string() },
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
            let cam = config
                .require_camera(name)?;
            vec![cam]
        }
        None => config.cameras.iter().collect(),
    };

    if cameras_to_test.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No cameras configured. Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.");
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

fn cmd_add(
    host: &str,
    name: Option<&str>,
    camera_type_arg: CameraTypeArg,
    username: &str,
    password: Option<&str>,
    rtsp_port: u16,
    go2rtc_stream: Option<&str>,
) -> Result<()> {
    let mut config = config::Config::load()?;

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
        bail!("a camera named '{}' already exists in config", resolved_name);
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
        frigate_name: None,
    };

    config.cameras.push(new_camera);

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content)
        .with_context(|| format!("writing {}", path.display()))?;

    println!(
        "Added camera '{}' ({} @ {})",
        resolved_name, camera_type, host
    );

    Ok(())
}

fn cmd_remove(name: &str, yes: bool) -> Result<()> {
    let mut config = config::Config::load()?;

    // Validate the camera exists (require_camera gives a good error message)
    config.require_camera(name)?;
    let pos = config
        .cameras
        .iter()
        .position(|c| c.name == name)
        .expect("require_camera succeeded so position must exist");

    let cam = &config.cameras[pos];
    println!(
        "Will remove: '{}' ({} @ {})",
        cam.name, cam.camera_type, cam.host
    );

    if !yes {
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

    config.cameras.remove(pos);

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content)
        .with_context(|| format!("writing {}", path.display()))?;

    println!("Removed camera '{}'.", name);

    Ok(())
}

fn cmd_rename(old_name: &str, new_name: &str) -> Result<()> {
    let mut config = config::Config::load()?;

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

    let mut updated_frigate = false;
    if let Some(ref fname) = cam.frigate_name.clone()
        && *fname == old_auto
    {
        cam.frigate_name = Some(new_auto.clone());
        updated_frigate = true;
    }

    let path = config::Config::config_path()?;
    let content = toml::to_string_pretty(&config).context("serializing config")?;
    std::fs::write(&path, content)
        .with_context(|| format!("writing {}", path.display()))?;

    println!("Renamed camera '{}' -> '{}'", old_name, new_name);
    if updated_go2rtc {
        println!("  go2rtc_stream: '{}' -> '{}'", old_auto, new_auto);
    }
    if updated_frigate {
        println!("  frigate_name:  '{}' -> '{}'", old_auto, new_auto);
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
}
