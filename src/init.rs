use std::io::{self, Write};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{CameraConfig, CameraType, Config, Go2rtcConfig};
use crate::discovery::{DiscoveredCamera, discover_cameras};

// ── name generation ──────────────────────────────────────────────────────────

/// Generate a sensible default camera name from model string and IP address.
///
/// "Reolink Video Doorbell WiFi" at 192.168.1.215 → "doorbell-215"
/// Unknown at 192.168.1.97 → "camera-97"
pub(crate) fn default_camera_name(model: Option<&str>, address: &str) -> String {
    let last_octet = address
        .rsplit('.')
        .next()
        .unwrap_or(address)
        .split(':') // strip port if present
        .next()
        .unwrap_or(address);

    if let Some(model) = model {
        let lower = model.to_lowercase();
        // Look for meaningful words to use as a prefix, skipping brand names.
        let skip = ["reolink", "tp-link", "tapo", "hikvision", "dahua", "axis"];
        let meaningful: Vec<&str> = lower
            .split_whitespace()
            .filter(|w| !skip.contains(w) && w.len() > 2)
            .collect();

        if let Some(word) = meaningful.first() {
            return format!("{}-{}", word, last_octet);
        }
    }

    format!("camera-{}", last_octet)
}

// ── camera type inference ─────────────────────────────────────────────────────

/// Infer camera type from manufacturer string, if possible.
pub(crate) fn infer_camera_type(manufacturer: Option<&str>) -> Option<CameraType> {
    let m = manufacturer?.to_lowercase();
    if m.contains("reolink") {
        Some(CameraType::Reolink)
    } else if m.contains("tp-link") || m.contains("tapo") {
        Some(CameraType::Tapo)
    } else {
        None
    }
}

// ── stdin helpers ─────────────────────────────────────────────────────────────

/// Print prompt then read a line from stdin (trims trailing newline).
fn prompt(msg: &str) -> Result<String> {
    print!("{}", msg);
    io::stdout().flush().context("flush stdout")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("read from stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Ask a yes/no question. `default_yes` controls what pressing Enter means.
fn ask_yes_no(question: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let answer = prompt(&format!("{} {}: ", question, hint))?;
    Ok(match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default_yes,
        _ => default_yes,
    })
}

/// Read a line with a default value shown in brackets.
fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    let raw = prompt(&format!("{} [{}]: ", label, default))?;
    if raw.trim().is_empty() {
        Ok(default.to_string())
    } else {
        Ok(raw.trim().to_string())
    }
}

/// Read a line without echoing (best-effort; falls back to normal read).
fn prompt_password(label: &str) -> Result<String> {
    print!("{}: ", label);
    io::stdout().flush().context("flush stdout")?;
    if std::io::IsTerminal::is_terminal(&io::stdin()) {
        return rpassword::read_password().context("read password from terminal");
    }
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("read password from stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

// ── interactive camera prompts ────────────────────────────────────────────────

/// Interactively ask the user about a discovered camera.
/// Returns `Some(CameraConfig)` if the user wants to add it, `None` otherwise.
pub(crate) fn prompt_for_camera(cam: &DiscoveredCamera) -> Result<Option<CameraConfig>> {
    let display = match (&cam.manufacturer, &cam.model) {
        (Some(mfr), Some(mdl)) => format!("{} {}", mfr, mdl),
        (Some(mfr), None) => mfr.clone(),
        (None, Some(mdl)) => mdl.clone(),
        (None, None) => "Unknown camera".to_string(),
    };

    println!();
    println!("Found: {} ({})", cam.address, display);

    if !ask_yes_no("Add this camera?", true)? {
        return Ok(None);
    }

    let suggested_name = default_camera_name(cam.model.as_deref(), &cam.address);
    let name = prompt_with_default("Name", &suggested_name)?;

    // Determine camera type: infer or ask.
    let camera_type = match infer_camera_type(cam.manufacturer.as_deref()) {
        Some(t) => {
            println!("Camera type: {} (detected from manufacturer)", t);
            t
        }
        None => {
            println!("Camera type unknown. Options: tapo, reolink");
            loop {
                let raw = prompt("Type: ")?;
                match raw.trim().to_lowercase().as_str() {
                    "tapo" => break CameraType::Tapo,
                    "reolink" => break CameraType::Reolink,
                    _ => println!("Please enter 'tapo' or 'reolink'."),
                }
            }
        }
    };

    let username = prompt_with_default("Username", "admin")?;
    let password = prompt_password("Password")?;

    Ok(Some(CameraConfig {
        name,
        camera_type,
        host: cam.address.clone(),
        rtsp_port: 554,
        username: if username.is_empty() {
            None
        } else {
            Some(username)
        },
        password: if password.is_empty() {
            None
        } else {
            Some(password)
        },
        go2rtc_stream: None,
        onvif_port: None,
        main_stream: None,
        sub_stream: None,
        onvif_username: None,
        onvif_password: None,
    }))
}

// ── auto-mode camera builder ──────────────────────────────────────────────────

/// Build a CameraConfig from a discovered camera using defaults (no passwords).
pub(crate) fn auto_camera_config(cam: &DiscoveredCamera) -> Option<CameraConfig> {
    let camera_type = infer_camera_type(cam.manufacturer.as_deref())?;
    let name = default_camera_name(cam.model.as_deref(), &cam.address);

    Some(CameraConfig {
        name,
        camera_type,
        host: cam.address.clone(),
        rtsp_port: 554,
        username: Some("admin".to_string()),
        password: None,
        go2rtc_stream: None,
        onvif_port: None,
        main_stream: None,
        sub_stream: None,
        onvif_username: None,
        onvif_password: None,
    })
}

// ── go2rtc prompt ────────────────────────────────────────────────────────────

fn prompt_go2rtc() -> Result<Option<Go2rtcConfig>> {
    println!();
    if !ask_yes_no("Do you have a go2rtc server?", false)? {
        return Ok(None);
    }
    let host = prompt_with_default("go2rtc host", "localhost")?;
    let port_str = prompt_with_default("go2rtc RTSP port", "8554")?;
    let port: u16 = port_str
        .parse()
        .context("go2rtc port must be a number 1-65535")?;
    Ok(Some(Go2rtcConfig { host, port }))
}

// ── config file writing ───────────────────────────────────────────────────────

fn write_config(config: &Config, path: &std::path::Path) -> Result<()> {
    config.save_to(path)
}

// ── print summary ─────────────────────────────────────────────────────────────

fn print_summary(config: &Config, path: &std::path::Path) {
    println!();
    println!("Configuration written to: {}", path.display());
    println!();

    if config.cameras.is_empty() {
        println!("No cameras configured.");
    } else {
        println!("Cameras:");
        for cam in &config.cameras {
            println!("  - {} ({}) at {}", cam.name, cam.camera_type, cam.host);
        }
    }

    if let Some(g) = &config.go2rtc {
        println!("go2rtc: {}:{}", g.host, g.port);
    }
}

// ── public entry points ───────────────────────────────────────────────────────

pub async fn run_init(auto: bool) -> Result<()> {
    run_init_at(auto, None).await
}

pub async fn run_init_at(auto: bool, custom_path: Option<&std::path::Path>) -> Result<()> {
    let config_path = Config::resolved_path(custom_path)?;

    if config_path.exists() && !Config::load(Some(&config_path))?.cameras.is_empty() {
        println!("Config already exists at: {}", config_path.display());
        if auto {
            println!("Use `ipcam discover` to find and add new cameras.");
            return Ok(());
        }
        let overwrite = ask_yes_no(
            "Start fresh? (No = use `ipcam discover` to add cameras)",
            false,
        )?;
        if !overwrite {
            println!();
            println!("To add new cameras: ipcam discover");
            println!("To add cameras on a VLAN: ipcam discover --subnet 10.10.20.0/24");
            return Ok(());
        }
    }

    // First-time setup: discover cameras, then ask about go2rtc.
    println!();
    println!("Scanning network for cameras (this may take a few seconds)...");
    let discovered = discover_cameras(Duration::from_secs(5), None).await?;

    if discovered.is_empty() {
        println!("No cameras found on the network.");
        println!("You can add cameras manually: ipcam add --host <IP> --type <tapo|reolink>");
        println!("Or scan a specific subnet: ipcam discover --subnet 10.10.20.0/24");
    } else {
        println!("Found {} camera(s) on the network.", discovered.len());
    }

    let mut cameras: Vec<CameraConfig> = Vec::new();

    if auto {
        for cam in &discovered {
            match auto_camera_config(cam) {
                Some(c) => {
                    println!("  + {} ({}) at {}", c.name, c.camera_type, cam.address);
                    cameras.push(c);
                }
                None => {
                    println!(
                        "  ? {} — could not detect type (manufacturer: {}), skipping",
                        cam.address,
                        cam.manufacturer.as_deref().unwrap_or("unknown")
                    );
                }
            }
        }
    } else {
        for cam in &discovered {
            if let Some(c) = prompt_for_camera(cam)? {
                cameras.push(c);
            }
        }
    }

    let go2rtc = if auto { None } else { prompt_go2rtc()? };

    let config = Config { cameras, go2rtc };

    write_config(&config, &config_path)?;
    print_summary(&config, &config_path);

    Ok(())
}

/// Update only the credentials for one camera, preserving every other camera
/// and advanced stream setting. This is the focused recovery path used by the
/// TUI when a selected camera rejects its credentials.
pub fn run_credential_update(
    camera_name: &str,
    custom_path: Option<&std::path::Path>,
) -> Result<()> {
    let path = Config::resolved_path(custom_path)?;
    let mut config = Config::load(custom_path)?;
    let camera = config
        .cameras
        .iter_mut()
        .find(|camera| camera.name == camera_name)
        .with_context(|| format!("camera '{camera_name}' is no longer configured"))?;

    println!();
    println!(
        "Update credentials for '{}' at {}",
        camera.name, camera.host
    );
    println!("Press Enter to keep a saved value.");
    println!();
    let username = prompt_with_default("Username", camera.username.as_deref().unwrap_or("admin"))?;
    let password = prompt_password(if camera.password.is_some() {
        "Password [saved — Enter keeps it]"
    } else {
        "Password"
    })?;
    camera.username = (!username.is_empty()).then_some(username);
    if !password.is_empty() {
        camera.password = Some(password);
    }

    if camera.onvif_username.is_some() || camera.onvif_password.is_some() {
        println!();
        println!("This camera has separate ONVIF credentials.");
        let onvif_username = prompt_with_default(
            "ONVIF username",
            camera
                .onvif_username
                .as_deref()
                .or(camera.username.as_deref())
                .unwrap_or("admin"),
        )?;
        let onvif_password = prompt_password(if camera.onvif_password.is_some() {
            "ONVIF password [saved — Enter keeps it]"
        } else {
            "ONVIF password"
        })?;
        camera.onvif_username = (!onvif_username.is_empty()).then_some(onvif_username);
        if !onvif_password.is_empty() {
            camera.onvif_password = Some(onvif_password);
        }
    }

    config.save_to(&path)?;
    println!("Credentials saved securely. Returning to the dashboard…");
    Ok(())
}
