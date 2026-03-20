use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub go2rtc: Option<Go2rtcConfig>,
    #[serde(default)]
    pub frigate: Option<FrigateConfig>,
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Go2rtcConfig {
    pub host: String,
    #[serde(default = "default_go2rtc_port")]
    pub port: u16,
}

impl Go2rtcConfig {
    pub fn rtsp_url(&self, stream_name: &str) -> String {
        format!("rtsp://{}:{}/{}", self.host, self.port, stream_name)
    }
}

fn default_go2rtc_port() -> u16 {
    8554
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrigateConfig {
    pub host: String,
    #[serde(default = "default_frigate_port")]
    pub port: u16,
}

impl FrigateConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

fn default_frigate_port() -> u16 {
    5001
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub camera_type: CameraType,
    pub host: String,
    #[serde(default = "default_rtsp_port")]
    pub rtsp_port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// go2rtc stream name for cameras behind a restream proxy
    pub go2rtc_stream: Option<String>,
    /// ONVIF port override (default: 2020 for Tapo, 8000 for Reolink)
    pub onvif_port: Option<u16>,
    /// Frigate camera name (Frigate uses underscores, e.g. "front_door")
    pub frigate_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CameraType {
    Tapo,
    Reolink,
}

impl std::fmt::Display for CameraType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tapo => write!(f, "tapo"),
            Self::Reolink => write!(f, "reolink"),
        }
    }
}

impl CameraConfig {
    /// Returns the ONVIF port, using the explicit override or a per-vendor default.
    pub fn onvif_port(&self) -> u16 {
        self.onvif_port.unwrap_or(match self.camera_type {
            CameraType::Tapo => 2020,
            CameraType::Reolink => 8000,
        })
    }

    /// Returns the Frigate camera name, falling back to the config name with
    /// hyphens replaced by underscores.
    pub fn frigate_name(&self) -> String {
        self.frigate_name
            .clone()
            .unwrap_or_else(|| self.name.replace('-', "_"))
    }
}

fn default_rtsp_port() -> u16 {
    554
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self {
                go2rtc: None,
                frigate: None,
                cameras: Vec::new(),
            });
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&content).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("could not determine config directory")?
            .join("ipcam");
        Ok(config_dir.join("config.toml"))
    }

    pub fn find_camera(&self, name: &str) -> Option<&CameraConfig> {
        self.cameras.iter().find(|c| c.name == name)
    }
}
