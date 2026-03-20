use std::time::Instant;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine;
use sha1::{Digest, Sha1};

use crate::camera::{
    Camera, CameraInfo, HealthStatus, ImageFormat, MotionStatus, PtzDirection, Snapshot,
    StreamQuality,
};
use crate::config::{CameraConfig, Go2rtcConfig};

pub struct TapoCamera {
    name: String,
    host: String,
    rtsp_port: u16,
    onvif_port: u16,
    username: String,
    password: String,
    go2rtc_stream: Option<String>,
    go2rtc: Option<Go2rtcConfig>,
    client: reqwest::Client,
}

impl TapoCamera {
    pub fn new(config: &CameraConfig, go2rtc: Option<&Go2rtcConfig>) -> Result<Self> {
        let username = config.username.clone().unwrap_or_default();
        let password = config.password.clone().unwrap_or_default();

        Ok(Self {
            name: config.name.clone(),
            host: config.host.clone(),
            rtsp_port: config.rtsp_port,
            onvif_port: config.onvif_port(),
            username,
            password,
            go2rtc_stream: config.go2rtc_stream.clone(),
            go2rtc: go2rtc.cloned(),
            client: reqwest::Client::new(),
        })
    }

    fn ptz_url(&self) -> String {
        format!("http://{}:{}/onvif/ptz_service", self.host, self.onvif_port)
    }

    /// Build a SOAP envelope with WS-Security UsernameToken using Password Digest.
    /// Digest = Base64(SHA1(nonce + created + password))
    fn soap_envelope(&self, body: &str) -> String {
        let nonce_bytes: [u8; 16] = rand::random();
        let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);
        let created = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let mut hasher = Sha1::new();
        hasher.update(nonce_bytes);
        hasher.update(created.as_bytes());
        hasher.update(self.password.as_bytes());
        let digest = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"
            xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
  <s:Header>
    <wsse:Security>
      <wsse:UsernameToken>
        <wsse:Username>{username}</wsse:Username>
        <wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</wsse:Password>
        <wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{nonce}</wsse:Nonce>
        <wsu:Created>{created}</wsu:Created>
      </wsse:UsernameToken>
    </wsse:Security>
  </s:Header>
  <s:Body>
    {body}
  </s:Body>
</s:Envelope>"#,
            username = self.username,
            digest = digest,
            nonce = nonce_b64,
            created = created,
            body = body,
        )
    }

    async fn send_ptz_soap(&self, body: &str) -> Result<()> {
        let envelope = self.soap_envelope(body);
        let resp = self
            .client
            .post(self.ptz_url())
            .header("Content-Type", "application/soap+xml; charset=utf-8")
            .body(envelope)
            .send()
            .await
            .context("failed to connect to ONVIF PTZ service")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("ONVIF PTZ request failed ({}): {}", status, text);
        }
        Ok(())
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

    async fn is_reachable(&self) -> HealthStatus {
        let start = Instant::now();
        let addr = format!("{}:{}", self.host, self.rtsp_port);
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_)) => HealthStatus {
                online: true,
                detail: addr,
                latency: start.elapsed(),
            },
            Ok(Err(e)) => HealthStatus {
                online: false,
                detail: e.to_string(),
                latency: start.elapsed(),
            },
            Err(_) => HealthStatus {
                online: false,
                detail: "connection timed out".to_string(),
                latency: start.elapsed(),
            },
        }
    }

    async fn ptz_move(&self, direction: PtzDirection, speed: f32) -> Result<()> {
        let (pan, tilt) = direction.velocity(speed);
        let body = format!(
            r#"<tptz:ContinuousMove xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:Velocity>
    <tt:PanTilt x="{pan}" y="{tilt}" xmlns:tt="http://www.onvif.org/ver10/schema"/>
  </tptz:Velocity>
</tptz:ContinuousMove>"#,
        );
        self.send_ptz_soap(&body).await
    }

    async fn ptz_stop(&self) -> Result<()> {
        let body = r#"<tptz:Stop xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:PanTilt>true</tptz:PanTilt>
  <tptz:Zoom>true</tptz:Zoom>
</tptz:Stop>"#;
        self.send_ptz_soap(body).await
    }

    async fn ptz_goto_preset(&self, preset: u32) -> Result<()> {
        let body = format!(
            r#"<tptz:GotoPreset xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:PresetToken>{preset}</tptz:PresetToken>
</tptz:GotoPreset>"#,
        );
        self.send_ptz_soap(&body).await
    }
}
