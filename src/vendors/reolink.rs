use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::camera::{Camera, CameraInfo, ImageFormat, MotionStatus, Snapshot, StreamQuality};
use crate::config::CameraConfig;

pub struct ReolinkCamera {
    name: String,
    host: String,
    rtsp_port: u16,
    username: String,
    password: String,
    client: reqwest::Client,
}

impl ReolinkCamera {
    pub fn new(config: &CameraConfig) -> Result<Self> {
        let username = config
            .username
            .clone()
            .unwrap_or_else(|| "admin".to_string());
        let password = config.password.clone().unwrap_or_default();

        Ok(Self {
            name: config.name.clone(),
            host: config.host.clone(),
            rtsp_port: config.rtsp_port,
            username,
            password,
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()?,
        })
    }

    fn api_url(&self, cmd: &str) -> String {
        format!(
            "https://{}/api.cgi?cmd={}&user={}&password={}",
            self.host, cmd, self.username, self.password
        )
    }
}

#[async_trait]
impl Camera for ReolinkCamera {
    async fn info(&self) -> Result<CameraInfo> {
        let url = self.api_url("GetDevInfo");
        let body = serde_json::json!([{
            "cmd": "GetDevInfo",
            "action": 0,
            "param": {}
        }]);

        let resp = self.client.post(&url).json(&body).send().await?;
        let data: serde_json::Value = resp.json().await?;

        let dev_info = &data[0]["value"]["DevInfo"];
        Ok(CameraInfo {
            name: self.name.clone(),
            model: dev_info["model"].as_str().map(String::from),
            firmware: dev_info["firmVer"].as_str().map(String::from),
            host: self.host.clone(),
        })
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let url = format!(
            "https://{}/cgi-bin/api.cgi?cmd=Snap&channel=0&user={}&password={}",
            self.host, self.username, self.password,
        );

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            bail!(
                "snapshot request failed with status {} for camera '{}'",
                resp.status(),
                self.name,
            );
        }

        let data = resp.bytes().await?.to_vec();
        Ok(Snapshot {
            camera_name: self.name.clone(),
            timestamp: chrono::Utc::now(),
            data,
            format: ImageFormat::Jpeg,
        })
    }

    fn rtsp_url(&self, quality: StreamQuality) -> String {
        let stream = match quality {
            StreamQuality::Main => "h264Preview_01_main",
            StreamQuality::Sub => "h264Preview_01_sub",
        };
        format!(
            "rtsp://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.rtsp_port, stream
        )
    }

    async fn motion_status(&self) -> Result<MotionStatus> {
        let url = self.api_url("GetMdState");
        let body = serde_json::json!([{
            "cmd": "GetMdState",
            "action": 0,
            "param": { "channel": 0 }
        }]);

        let resp = self.client.post(&url).json(&body).send().await?;
        let data: serde_json::Value = resp.json().await?;

        let state = data[0]["value"]["state"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("unexpected GetMdState response: {}", data))?;

        Ok(MotionStatus {
            detected: state != 0,
            timestamp: Some(chrono::Utc::now()),
        })
    }
}
