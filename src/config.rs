use std::path::{Path, PathBuf};

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
    /// Custom RTSP main stream path (default varies by vendor)
    pub main_stream: Option<String>,
    /// Custom RTSP sub stream path (default varies by vendor)
    pub sub_stream: Option<String>,
    /// Separate ONVIF username (if different from RTSP credentials)
    pub onvif_username: Option<String>,
    /// Separate ONVIF password (if different from RTSP credentials)
    pub onvif_password: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CameraType {
    Tapo,
    Reolink,
    Onvif,
}

impl std::fmt::Display for CameraType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tapo => write!(f, "tapo"),
            Self::Reolink => write!(f, "reolink"),
            Self::Onvif => write!(f, "onvif"),
        }
    }
}

impl CameraConfig {
    /// Returns the ONVIF port, using the explicit override or a per-vendor default.
    pub fn onvif_port(&self) -> u16 {
        self.onvif_port.unwrap_or(match self.camera_type {
            CameraType::Tapo => 2020,
            CameraType::Reolink => 8000,
            CameraType::Onvif => 80,
        })
    }

    /// Returns ONVIF credentials, falling back to the RTSP credentials.
    pub fn onvif_credentials(&self) -> (String, String) {
        let username = self
            .onvif_username
            .clone()
            .or_else(|| self.username.clone())
            .unwrap_or_default();
        let password = self
            .onvif_password
            .clone()
            .or_else(|| self.password.clone())
            .unwrap_or_default();
        (username, password)
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
    pub fn load(custom_path: Option<&Path>) -> Result<Self> {
        let path = match custom_path {
            Some(p) => p.to_path_buf(),
            None => Self::config_path()?,
        };
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

    /// Look up a camera by name, returning a descriptive error that lists
    /// available cameras when the name is not found.
    pub fn require_camera(&self, name: &str) -> Result<&CameraConfig> {
        self.find_camera(name).ok_or_else(|| {
            if self.cameras.is_empty() {
                anyhow::anyhow!(
                    "camera '{}' not found (no cameras configured). \
                     Run `ipcam init` to discover cameras, or `ipcam add` to add one manually.",
                    name
                )
            } else {
                let names: Vec<&str> = self.cameras.iter().map(|c| c.name.as_str()).collect();
                anyhow::anyhow!(
                    "camera '{}' not found. Available cameras: {}",
                    name,
                    names.join(", ")
                )
            }
        })
    }

    /// Returns true if the config file exists on disk.
    pub fn config_exists() -> Result<bool> {
        Ok(Self::config_path()?.exists())
    }

    pub fn migrate_if_needed() -> Result<bool> {
        let new_path = Self::config_path()?;
        if new_path.exists() {
            return Ok(false);
        }

        let config_dir = dirs::config_dir().context("could not determine config directory")?;
        let candidates = [
            config_dir.join("camctl").join("config.toml"),
            config_dir.join("camera-cli").join("config.toml"),
        ];

        for old_path in &candidates {
            if old_path.exists() {
                if let Some(parent) = new_path.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating directory {}", parent.display()))?;
                }
                std::fs::copy(old_path, &new_path).with_context(|| {
                    format!(
                        "copying {} to {}",
                        old_path.display(),
                        new_path.display()
                    )
                })?;
                println!(
                    "Migrated config from {} to {}",
                    old_path.display(),
                    new_path.display()
                );
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config_toml() -> &'static str {
        r#"
[go2rtc]
host = "192.168.1.180"
port = 8554

[frigate]
host = "192.168.1.180"
port = 5001

[[cameras]]
name = "front-door"
type = "reolink"
host = "192.168.1.215"
rtsp_port = 554
username = "admin"
password = "secret"
onvif_port = 8000
frigate_name = "front_door"

[[cameras]]
name = "kids-room"
type = "tapo"
host = "192.168.1.97"
username = "user"
password = "pass"
go2rtc_stream = "kids_room"
"#
    }

    fn make_camera(camera_type: CameraType) -> CameraConfig {
        CameraConfig {
            name: "test-cam".to_string(),
            camera_type,
            host: "10.0.0.1".to_string(),
            rtsp_port: 554,
            username: None,
            password: None,
            go2rtc_stream: None,
            onvif_port: None,
            frigate_name: None,
            main_stream: None,
            sub_stream: None,
            onvif_username: None,
            onvif_password: None,
        }
    }

    // --- TOML parsing ---

    #[test]
    fn parse_full_config() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        assert!(config.go2rtc.is_some());
        assert!(config.frigate.is_some());
        assert_eq!(config.cameras.len(), 2);
    }

    #[test]
    fn parse_camera_fields() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        let cam = &config.cameras[0];
        assert_eq!(cam.name, "front-door");
        assert_eq!(cam.camera_type, CameraType::Reolink);
        assert_eq!(cam.host, "192.168.1.215");
        assert_eq!(cam.rtsp_port, 554);
        assert_eq!(cam.username.as_deref(), Some("admin"));
        assert_eq!(cam.password.as_deref(), Some("secret"));
        assert_eq!(cam.onvif_port, Some(8000));
        assert_eq!(cam.frigate_name.as_deref(), Some("front_door"));
    }

    #[test]
    fn parse_optional_fields_absent() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        let cam = &config.cameras[1];
        assert_eq!(cam.onvif_port, None);
        assert_eq!(cam.frigate_name, None);
        assert_eq!(cam.go2rtc_stream.as_deref(), Some("kids_room"));
    }

    #[test]
    fn empty_config_no_cameras() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.cameras.is_empty());
        assert!(config.go2rtc.is_none());
        assert!(config.frigate.is_none());
    }

    #[test]
    fn camera_type_deserialize_lowercase() {
        let toml_str = r#"
[[cameras]]
name = "test"
type = "tapo"
host = "10.0.0.1"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.cameras[0].camera_type, CameraType::Tapo);
    }

    // --- Default values ---

    #[test]
    fn default_rtsp_port_is_554() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        // Second camera omits rtsp_port
        assert_eq!(config.cameras[1].rtsp_port, 554);
    }

    #[test]
    fn default_go2rtc_port_is_8554() {
        let config: Config = toml::from_str(
            r#"
[go2rtc]
host = "10.0.0.1"
"#,
        )
        .unwrap();
        assert_eq!(config.go2rtc.unwrap().port, 8554);
    }

    #[test]
    fn default_frigate_port_is_5001() {
        let config: Config = toml::from_str(
            r#"
[frigate]
host = "10.0.0.1"
"#,
        )
        .unwrap();
        assert_eq!(config.frigate.unwrap().port, 5001);
    }

    // --- onvif_port() ---

    #[test]
    fn onvif_port_tapo_default() {
        assert_eq!(make_camera(CameraType::Tapo).onvif_port(), 2020);
    }

    #[test]
    fn onvif_port_reolink_default() {
        assert_eq!(make_camera(CameraType::Reolink).onvif_port(), 8000);
    }

    #[test]
    fn onvif_port_explicit_override() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.onvif_port = Some(9999);
        assert_eq!(cam.onvif_port(), 9999);
    }

    // --- onvif_credentials() ---

    #[test]
    fn onvif_credentials_falls_back_to_rtsp() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.username = Some("rtsp_user".to_string());
        cam.password = Some("rtsp_pass".to_string());
        let (user, pass) = cam.onvif_credentials();
        assert_eq!(user, "rtsp_user");
        assert_eq!(pass, "rtsp_pass");
    }

    #[test]
    fn onvif_credentials_uses_dedicated_when_set() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.username = Some("rtsp_user".to_string());
        cam.password = Some("rtsp_pass".to_string());
        cam.onvif_username = Some("onvif_user".to_string());
        cam.onvif_password = Some("onvif_pass".to_string());
        let (user, pass) = cam.onvif_credentials();
        assert_eq!(user, "onvif_user");
        assert_eq!(pass, "onvif_pass");
    }

    #[test]
    fn onvif_credentials_partial_override() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.username = Some("rtsp_user".to_string());
        cam.password = Some("rtsp_pass".to_string());
        cam.onvif_username = Some("onvif_user".to_string());
        // onvif_password not set, should fall back to rtsp password
        let (user, pass) = cam.onvif_credentials();
        assert_eq!(user, "onvif_user");
        assert_eq!(pass, "rtsp_pass");
    }

    // --- frigate_name() ---

    #[test]
    fn frigate_name_from_explicit_field() {
        let mut cam = make_camera(CameraType::Reolink);
        cam.frigate_name = Some("custom_name".to_string());
        assert_eq!(cam.frigate_name(), "custom_name");
    }

    #[test]
    fn frigate_name_converts_hyphens_to_underscores() {
        let cam = make_camera(CameraType::Reolink);
        // name is "test-cam", should become "test_cam"
        assert_eq!(cam.frigate_name(), "test_cam");
    }

    #[test]
    fn frigate_name_no_hyphens_unchanged() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.name = "backyard".to_string();
        assert_eq!(cam.frigate_name(), "backyard");
    }

    #[test]
    fn frigate_name_multiple_hyphens() {
        let mut cam = make_camera(CameraType::Tapo);
        cam.name = "front-door-left".to_string();
        assert_eq!(cam.frigate_name(), "front_door_left");
    }

    // --- find_camera / require_camera ---

    #[test]
    fn find_camera_by_name() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        let found = config.find_camera("kids-room");
        assert!(found.is_some());
        assert_eq!(found.unwrap().host, "192.168.1.97");
    }

    #[test]
    fn find_camera_missing_returns_none() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        assert!(config.find_camera("nonexistent").is_none());
    }

    #[test]
    fn require_camera_found() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        let cam = config.require_camera("front-door").unwrap();
        assert_eq!(cam.host, "192.168.1.215");
    }

    #[test]
    fn require_camera_missing_lists_available() {
        let config: Config = toml::from_str(sample_config_toml()).unwrap();
        let err = config.require_camera("nope").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"));
        assert!(msg.contains("front-door"));
        assert!(msg.contains("kids-room"));
    }

    #[test]
    fn require_camera_empty_config_suggests_init() {
        let config: Config = toml::from_str("").unwrap();
        let err = config.require_camera("any").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no cameras configured"));
    }

    // --- Go2rtcConfig ---

    #[test]
    fn go2rtc_rtsp_url() {
        let go2rtc = Go2rtcConfig {
            host: "192.168.1.180".to_string(),
            port: 8554,
        };
        assert_eq!(
            go2rtc.rtsp_url("kids_room"),
            "rtsp://192.168.1.180:8554/kids_room"
        );
    }

    #[test]
    fn go2rtc_rtsp_url_custom_port() {
        let go2rtc = Go2rtcConfig {
            host: "10.0.0.5".to_string(),
            port: 9000,
        };
        assert_eq!(
            go2rtc.rtsp_url("stream1"),
            "rtsp://10.0.0.5:9000/stream1"
        );
    }

    // --- FrigateConfig ---

    #[test]
    fn frigate_base_url() {
        let frigate = FrigateConfig {
            host: "192.168.1.180".to_string(),
            port: 5001,
        };
        assert_eq!(frigate.base_url(), "http://192.168.1.180:5001");
    }

    #[test]
    fn frigate_base_url_custom_port() {
        let frigate = FrigateConfig {
            host: "10.0.0.1".to_string(),
            port: 8080,
        };
        assert_eq!(frigate.base_url(), "http://10.0.0.1:8080");
    }

    // --- config_path ---

    #[test]
    fn config_path_ends_with_ipcam() {
        let path = Config::config_path().unwrap();
        assert!(path.ends_with("ipcam/config.toml"));
    }

    // --- CameraType Display ---

    #[test]
    fn camera_type_display() {
        assert_eq!(CameraType::Tapo.to_string(), "tapo");
        assert_eq!(CameraType::Reolink.to_string(), "reolink");
        assert_eq!(CameraType::Onvif.to_string(), "onvif");
    }

    // --- Onvif camera type ---

    #[test]
    fn onvif_port_onvif_default() {
        assert_eq!(make_camera(CameraType::Onvif).onvif_port(), 80);
    }

    #[test]
    fn camera_type_deserialize_onvif() {
        let toml_str = r#"
[[cameras]]
name = "garage"
type = "onvif"
host = "192.168.1.50"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.cameras[0].camera_type, CameraType::Onvif);
    }

    #[test]
    fn parse_custom_stream_paths() {
        let toml_str = r#"
[[cameras]]
name = "garage"
type = "onvif"
host = "192.168.1.50"
main_stream = "Streaming/Channels/101"
sub_stream = "Streaming/Channels/102"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let cam = &config.cameras[0];
        assert_eq!(cam.main_stream.as_deref(), Some("Streaming/Channels/101"));
        assert_eq!(cam.sub_stream.as_deref(), Some("Streaming/Channels/102"));
    }

    #[test]
    fn stream_paths_optional() {
        let toml_str = r#"
[[cameras]]
name = "garage"
type = "onvif"
host = "192.168.1.50"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let cam = &config.cameras[0];
        assert!(cam.main_stream.is_none());
        assert!(cam.sub_stream.is_none());
    }
}
