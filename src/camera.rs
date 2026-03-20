use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    pub name: String,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub camera_name: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub data: Vec<u8>,
    pub format: ImageFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ImageFormat {
    Jpeg,
    Png,
}

impl ImageFormat {
    pub fn extension(&self) -> &str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
pub enum StreamQuality {
    Main,
    Sub,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionStatus {
    pub detected: bool,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub online: bool,
    pub detail: String,
    pub latency: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtzDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PtzDirection {
    /// Returns (pan, tilt) velocity values normalized to -1.0..1.0 range.
    pub fn velocity(&self, speed: f32) -> (f32, f32) {
        let v = speed.clamp(0.0, 1.0);
        match self {
            Self::Left => (-v, 0.0),
            Self::Right => (v, 0.0),
            Self::Up => (0.0, v),
            Self::Down => (0.0, -v),
        }
    }
}

#[async_trait]
pub trait Camera: Send + Sync {
    /// Get basic info about the camera
    async fn info(&self) -> Result<CameraInfo>;

    /// Capture a snapshot image
    async fn snapshot(&self) -> Result<Snapshot>;

    /// Get the RTSP URL for streaming
    fn rtsp_url(&self, quality: StreamQuality) -> String;

    /// Get current motion detection status
    async fn motion_status(&self) -> Result<MotionStatus>;

    /// Check if the camera is reachable
    async fn is_reachable(&self) -> HealthStatus;

    /// Start continuous PTZ movement in the given direction
    async fn ptz_move(&self, direction: PtzDirection, speed: f32) -> Result<()>;

    /// Stop PTZ movement
    async fn ptz_stop(&self) -> Result<()>;

    /// Go to a PTZ preset position
    async fn ptz_goto_preset(&self, preset: u32) -> Result<()>;
}
