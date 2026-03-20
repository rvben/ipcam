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
#[command(name = "camera-cli", about = "Manage IP cameras from the command line")]
struct Cli {
    /// Path to config file (default: ~/.config/camera-cli/config.toml)
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

        /// Directory to save snapshots when using --all or --every (default: current directory)
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Capture repeatedly at this interval (e.g. "30s", "5m", "1h"); saves timestamped files
        #[arg(long, conflicts_with = "all")]
        every: Option<String>,
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

    /// Show the config file path and status
    Config,

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
    ///   camera-cli completions zsh  > ~/.zfunc/_camera-cli
    ///   camera-cli completions bash > /etc/bash_completion.d/camera-cli
    ///   camera-cli completions fish > ~/.config/fish/completions/camera-cli.fish
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("camera_cli=info".parse().unwrap()),
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
        eprintln!("Hint: Check that the camera is online and reachable on the network.");
    } else if msg.contains("401")
        || msg.contains("403")
        || msg.contains("unauthorized")
        || msg.contains("authentication")
        || msg.contains("wrong password")
    {
        eprintln!();
        eprintln!("Hint: Check the username and password in your config file.");
    } else if msg.contains("not found in config") {
        eprintln!();
        eprintln!("Hint: Run `camera-cli list` to see configured cameras.");
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

    let config = config::Config::load()?;

    match cli.command {
        Command::List => cmd_list(&config, cli.json),
        Command::Info { camera } => cmd_info(&config, &camera, cli.json).await,
        Command::Snapshot {
            camera,
            output,
            all,
            output_dir,
            every,
        } => {
            if all {
                cmd_snapshot_all(&config, output_dir, cli.json).await
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
                cmd_snapshot(&config, &name, output).await
            }
        }
        Command::SnapshotAll { output_dir } => {
            cmd_snapshot_all(&config, output_dir, cli.json).await
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
        Command::Config => cmd_config(),
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
            println!("No cameras configured.");
            println!("Add cameras to: {}", config_path.display());
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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

async fn cmd_snapshot(config: &config::Config, name: &str, output: Option<PathBuf>) -> Result<()> {
    let cam_config = config
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    tracing::info!("capturing snapshot from '{}'...", name);
    let snapshot = cam.snapshot().await?;

    let path = output.unwrap_or_else(|| {
        let ts = snapshot.timestamp.format("%Y%m%d_%H%M%S");
        PathBuf::from(format!(
            "{}_{}.{}",
            snapshot.camera_name,
            ts,
            snapshot.format.extension()
        ))
    });

    std::fs::write(&path, &snapshot.data)?;
    println!("Saved snapshot to {}", path.display());
    Ok(())
}

async fn cmd_snapshot_all(
    config: &config::Config,
    output_dir: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if config.cameras.is_empty() {
        if json {
            println!("{}", serde_json::json!({"successes": [], "failures": []}));
        } else {
            println!("No cameras configured.");
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
                        let ts = snapshot.timestamp.format("%Y%m%d_%H%M%S");
                        let filename = format!(
                            "{}_{}.{}",
                            snapshot.camera_name,
                            ts,
                            snapshot.format.extension()
                        );
                        let path = dir.join(&filename);
                        match std::fs::write(&path, &snapshot.data) {
                            Ok(()) => (name, Ok(path)),
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

async fn cmd_stream(
    config: &config::Config,
    name: &str,
    quality: StreamQuality,
    output: Option<PathBuf>,
    duration: u64,
) -> Result<()> {
    let cam_config = config
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
    let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref())?;

    let (frames_dir, keep_frames) = match output_dir {
        Some(ref dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating frames directory: {}", dir.display()))?;
            (dir.clone(), true)
        }
        None => {
            let tmp = std::env::temp_dir().join(format!(
                "camera-cli-timelapse-{}",
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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
        .find_camera(name)
        .with_context(|| format!("camera '{}' not found in config", name))?;
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
                .find_camera(name)
                .with_context(|| format!("camera '{}' not found in config", name))?;
            vec![cam]
        }
        None => config.cameras.iter().collect(),
    };

    if cameras_to_check.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No cameras configured.");
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

async fn cmd_frigate(config: &config::Config, action: FrigateAction, json: bool) -> Result<()> {
    let frigate_config = config
        .frigate
        .as_ref()
        .context("no [frigate] section in config file")?;
    let client = frigate::FrigateClient::new(frigate_config);

    match action {
        FrigateAction::Events { camera, limit } => {
            // If user passes a camera-cli name, resolve its frigate_name
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
            // If user passes a camera-cli name, resolve its frigate_name
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

fn cmd_config() -> Result<()> {
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
