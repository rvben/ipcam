use std::io::{self, Write};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{CameraConfig, CameraType, Config, FrigateConfig, Go2rtcConfig};
use crate::discovery::{DiscoveredCamera, discover_cameras};

// ── name generation ──────────────────────────────────────────────────────────

/// Generate a sensible default camera name from model string and IP address.
///
/// "Reolink Video Doorbell WiFi" at 192.168.1.215 → "doorbell-215"
/// Unknown at 192.168.1.97 → "camera-97"
fn default_camera_name(model: Option<&str>, address: &str) -> String {
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
fn infer_camera_type(manufacturer: Option<&str>) -> Option<CameraType> {
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
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("read password from stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

// ── interactive camera prompts ────────────────────────────────────────────────

/// Interactively ask the user about a discovered camera.
/// Returns `Some(CameraConfig)` if the user wants to add it, `None` otherwise.
fn prompt_for_camera(cam: &DiscoveredCamera) -> Result<Option<CameraConfig>> {
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
        frigate_name: None,
        main_stream: None,
        sub_stream: None,
    }))
}

// ── auto-mode camera builder ──────────────────────────────────────────────────

/// Build a CameraConfig from a discovered camera using defaults (no passwords).
fn auto_camera_config(cam: &DiscoveredCamera) -> Option<CameraConfig> {
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
        frigate_name: None,
        main_stream: None,
        sub_stream: None,
    })
}

// ── go2rtc / Frigate prompts ──────────────────────────────────────────────────

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

fn prompt_frigate() -> Result<Option<FrigateConfig>> {
    println!();
    if !ask_yes_no("Do you have a Frigate NVR?", false)? {
        return Ok(None);
    }
    let host = prompt_with_default("Frigate host", "localhost")?;
    let port_str = prompt_with_default("Frigate port", "5001")?;
    let port: u16 = port_str
        .parse()
        .context("Frigate port must be a number 1-65535")?;
    Ok(Some(FrigateConfig { host, port }))
}

// ── config file writing ───────────────────────────────────────────────────────

fn write_config(config: &Config, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory: {}", parent.display()))?;
    }

    let toml = toml::to_string_pretty(config).context("serialising config to TOML")?;
    std::fs::write(path, &toml).with_context(|| format!("writing config to {}", path.display()))?;
    Ok(())
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

    if let Some(f) = &config.frigate {
        println!("Frigate: {}:{}", f.host, f.port);
    }
}

// ── public entry points ───────────────────────────────────────────────────────

pub async fn run_init(auto: bool) -> Result<()> {
    let config_path = Config::config_path()?;

    // Check if config already exists.
    let mut existing_cameras: Vec<CameraConfig> = Vec::new();
    let mut existing_go2rtc: Option<Go2rtcConfig> = None;
    let mut existing_frigate: Option<FrigateConfig> = None;

    if config_path.exists() {
        println!("Config already exists at: {}", config_path.display());

        if auto {
            // In auto mode silently overwrite.
        } else {
            let overwrite = ask_yes_no("Overwrite?", false)?;
            if !overwrite {
                // Offer to append cameras to existing config instead.
                let append = ask_yes_no("Append discovered cameras to existing config?", true)?;
                if !append {
                    println!("Aborted.");
                    return Ok(());
                }
                // Load existing config so we can append to it.
                let existing = Config::load()?;
                existing_cameras = existing.cameras;
                existing_go2rtc = existing.go2rtc;
                existing_frigate = existing.frigate;
            }
        }
    }

    // Run ONVIF discovery.
    println!();
    println!("Scanning network for cameras (this may take a few seconds)...");
    let discovered = discover_cameras(Duration::from_secs(5)).await?;

    if discovered.is_empty() {
        println!("No cameras found on the network.");
        if !auto {
            println!("You can add cameras manually to: {}", config_path.display());
        }
        return Ok(());
    }

    println!("Found {} camera(s) on the network.", discovered.len());

    let mut new_cameras: Vec<CameraConfig> = Vec::new();

    if auto {
        // Auto mode: add all cameras we can infer a type for.
        for cam in &discovered {
            match auto_camera_config(cam) {
                Some(c) => {
                    println!("  + {} ({}) at {}", c.name, c.camera_type, cam.address);
                    new_cameras.push(c);
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
        // Interactive mode: ask about each camera.
        for cam in &discovered {
            if let Some(c) = prompt_for_camera(cam)? {
                new_cameras.push(c);
            }
        }
    }

    // Merge with existing cameras (avoid duplicates by host).
    let existing_hosts: std::collections::HashSet<String> =
        existing_cameras.iter().map(|c| c.host.clone()).collect();

    let mut cameras = existing_cameras;
    for cam in new_cameras {
        if existing_hosts.contains(&cam.host) {
            println!(
                "Skipping {} (host {} already configured).",
                cam.name, cam.host
            );
        } else {
            cameras.push(cam);
        }
    }

    // Ask about go2rtc / Frigate (interactive only; skip in auto mode).
    let go2rtc = if auto {
        existing_go2rtc
    } else {
        prompt_go2rtc()?.or(existing_go2rtc)
    };

    let frigate = if auto {
        existing_frigate
    } else {
        prompt_frigate()?.or(existing_frigate)
    };

    let config = Config {
        cameras,
        go2rtc,
        frigate,
    };

    write_config(&config, &config_path)?;
    print_summary(&config, &config_path);

    Ok(())
}
