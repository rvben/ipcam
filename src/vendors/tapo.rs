use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::camera::{Camera, CameraInfo, ImageFormat, MotionStatus, Snapshot, StreamQuality};
use crate::config::{CameraConfig, Go2rtcConfig};

pub struct TapoCamera {
    name: String,
    host: String,
    rtsp_port: u16,
    username: String,
    password: String,
    go2rtc_stream: Option<String>,
    go2rtc: Option<Go2rtcConfig>,
}

impl TapoCamera {
    pub fn new(config: &CameraConfig, go2rtc: Option<&Go2rtcConfig>) -> Result<Self> {
        let username = config.username.clone().unwrap_or_default();
        let password = config.password.clone().unwrap_or_default();

        Ok(Self {
            name: config.name.clone(),
            host: config.host.clone(),
            rtsp_port: config.rtsp_port,
            username,
            password,
            go2rtc_stream: config.go2rtc_stream.clone(),
            go2rtc: go2rtc.cloned(),
        })
    }

    fn effective_rtsp_url(&self, quality: StreamQuality) -> String {
        if let (Some(stream), Some(go2rtc)) = (&self.go2rtc_stream, &self.go2rtc) {
            let suffix = match quality {
                StreamQuality::Main => "",
                StreamQuality::Sub => "_sub",
            };
            go2rtc.rtsp_url(&format!("{}{}", stream, suffix))
        } else {
            let stream = match quality {
                StreamQuality::Main => "stream1",
                StreamQuality::Sub => "stream2",
            };
            format!(
                "rtsp://{}:{}@{}:{}/{}",
                self.username, self.password, self.host, self.rtsp_port, stream
            )
        }
    }
}

#[async_trait]
impl Camera for TapoCamera {
    async fn info(&self) -> Result<CameraInfo> {
        Ok(CameraInfo {
            name: self.name.clone(),
            model: None,
            firmware: None,
            host: self.host.clone(),
        })
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let rtsp_url = self.effective_rtsp_url(StreamQuality::Sub);
        let tmp = std::env::temp_dir().join(format!("camera-cli-{}.jpg", self.name));

        let output = tokio::process::Command::new("ffmpeg")
            .args([
                "-rtsp_transport",
                "tcp",
                "-i",
                &rtsp_url,
                "-frames:v",
                "1",
                "-update",
                "1",
                "-y",
            ])
            .arg(&tmp)
            .output()
            .await
            .context("failed to run ffmpeg — is it installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "ffmpeg snapshot failed for camera '{}': {}",
                self.name,
                stderr.lines().last().unwrap_or("unknown error")
            );
        }

        let data = std::fs::read(&tmp)?;
        let _ = std::fs::remove_file(&tmp);

        Ok(Snapshot {
            camera_name: self.name.clone(),
            timestamp: chrono::Utc::now(),
            data,
            format: ImageFormat::Jpeg,
        })
    }

    fn rtsp_url(&self, quality: StreamQuality) -> String {
        self.effective_rtsp_url(quality)
    }

    async fn motion_status(&self) -> Result<MotionStatus> {
        bail!("motion detection is not supported for Tapo cameras")
    }
}
