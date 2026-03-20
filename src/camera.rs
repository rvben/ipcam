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
}
