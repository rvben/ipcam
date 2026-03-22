use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::camera::{
    Camera, CameraInfo, HealthStatus, MotionStatus, PtzDirection, Snapshot, StreamQuality,
};
use crate::config::{CameraConfig, Go2rtcConfig};

use super::soap;

pub struct TapoCamera {
    name: String,
    host: String,
    rtsp_port: u16,
    onvif_port: u16,
    username: String,
    password: String,
    onvif_username: String,
    onvif_password: String,
    go2rtc_stream: Option<String>,
    go2rtc: Option<Go2rtcConfig>,
    client: reqwest::Client,
}

impl TapoCamera {
    pub fn new(config: &CameraConfig, go2rtc: Option<&Go2rtcConfig>) -> Result<Self> {
        let username = config.username.clone().unwrap_or_default();
        let password = config.password.clone().unwrap_or_default();
        let (onvif_username, onvif_password) = config.onvif_credentials();

        Ok(Self {
            name: config.name.clone(),
            host: config.host.clone(),
            rtsp_port: config.rtsp_port,
            onvif_port: config.onvif_port(),
            username,
            password,
            onvif_username,
            onvif_password,
            go2rtc_stream: config.go2rtc_stream.clone(),
            go2rtc: go2rtc.cloned(),
            client: reqwest::Client::new(),
        })
    }

    fn ptz_url(&self) -> String {
        format!("http://{}:{}/onvif/ptz_service", self.host, self.onvif_port)
    }

    fn device_service_url(&self) -> String {
        format!(
            "http://{}:{}/onvif/device_service",
            self.host, self.onvif_port
        )
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
        soap::snapshot_with_fallback(
            &self.client,
            &self.name,
            &self.host,
            self.onvif_port,
            &self.onvif_username,
            &self.onvif_password,
            &rtsp_url,
        )
        .await
    }

    fn rtsp_url(&self, quality: StreamQuality) -> String {
        self.effective_rtsp_url(quality)
    }

    async fn motion_status(&self) -> Result<MotionStatus> {
        bail!("motion detection is not supported for Tapo cameras")
    }

    async fn is_reachable(&self) -> HealthStatus {
        soap::check_reachable(
            &self.client,
            &self.host,
            self.rtsp_port,
            &self.device_service_url(),
            &self.onvif_username,
            &self.onvif_password,
            "Tapo",
        )
        .await
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
        soap::send_ptz_soap(
            &self.client,
            &self.ptz_url(),
            &self.onvif_username,
            &self.onvif_password,
            &self.name,
            &self.host,
            &body,
        )
        .await
    }

    async fn ptz_stop(&self) -> Result<()> {
        let body = r#"<tptz:Stop xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:PanTilt>true</tptz:PanTilt>
  <tptz:Zoom>true</tptz:Zoom>
</tptz:Stop>"#;
        soap::send_ptz_soap(
            &self.client,
            &self.ptz_url(),
            &self.onvif_username,
            &self.onvif_password,
            &self.name,
            &self.host,
            body,
        )
        .await
    }

    async fn ptz_goto_preset(&self, preset: u32) -> Result<()> {
        let body = format!(
            r#"<tptz:GotoPreset xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:PresetToken>{preset}</tptz:PresetToken>
</tptz:GotoPreset>"#,
        );
        soap::send_ptz_soap(
            &self.client,
            &self.ptz_url(),
            &self.onvif_username,
            &self.onvif_password,
            &self.name,
            &self.host,
            &body,
        )
        .await
    }

    async fn ptz_zoom(&self, speed: f32) -> Result<()> {
        let body = format!(
            r#"<tptz:ContinuousMove xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
  <tptz:Velocity>
    <tt:Zoom x="{speed}" xmlns:tt="http://www.onvif.org/ver10/schema"/>
  </tptz:Velocity>
</tptz:ContinuousMove>"#,
        );
        soap::send_ptz_soap(
            &self.client,
            &self.ptz_url(),
            &self.onvif_username,
            &self.onvif_password,
            &self.name,
            &self.host,
            &body,
        )
        .await
    }

    async fn ptz_home(&self) -> Result<()> {
        let body = r#"<tptz:GotoHomePosition xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
  <tptz:ProfileToken>profile_1</tptz:ProfileToken>
</tptz:GotoHomePosition>"#;
        soap::send_ptz_soap(
            &self.client,
            &self.ptz_url(),
            &self.onvif_username,
            &self.onvif_password,
            &self.name,
            &self.host,
            body,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CameraConfig, CameraType, Go2rtcConfig};

    fn make_tapo_config() -> CameraConfig {
        CameraConfig {
            name: "kids-room".to_string(),
            camera_type: CameraType::Tapo,
            host: "192.168.1.97".to_string(),
            rtsp_port: 554,
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            go2rtc_stream: None,
            onvif_port: None,
            main_stream: None,
            sub_stream: None,
            onvif_username: None,
            onvif_password: None,
        }
    }

    fn make_go2rtc() -> Go2rtcConfig {
        Go2rtcConfig {
            host: "192.168.1.180".to_string(),
            port: 8554,
        }
    }

    #[test]
    fn direct_rtsp_url_main() {
        let config = make_tapo_config();
        let cam = TapoCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://user:pass@192.168.1.97:554/stream1"
        );
    }

    #[test]
    fn direct_rtsp_url_sub() {
        let config = make_tapo_config();
        let cam = TapoCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Sub),
            "rtsp://user:pass@192.168.1.97:554/stream2"
        );
    }

    #[test]
    fn go2rtc_restream_url_main() {
        let mut config = make_tapo_config();
        config.go2rtc_stream = Some("kids_room".to_string());
        let go2rtc = make_go2rtc();
        let cam = TapoCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://192.168.1.180:8554/kids_room"
        );
    }

    #[test]
    fn go2rtc_restream_url_sub() {
        let mut config = make_tapo_config();
        config.go2rtc_stream = Some("kids_room".to_string());
        let go2rtc = make_go2rtc();
        let cam = TapoCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Sub),
            "rtsp://192.168.1.180:8554/kids_room_sub"
        );
    }

    #[test]
    fn no_go2rtc_stream_ignores_go2rtc_config() {
        let config = make_tapo_config();
        let go2rtc = make_go2rtc();
        // go2rtc config provided but no go2rtc_stream on camera => direct RTSP
        let cam = TapoCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://user:pass@192.168.1.97:554/stream1"
        );
    }

    #[test]
    fn ptz_url_uses_onvif_port() {
        let config = make_tapo_config();
        let cam = TapoCamera::new(&config, None).unwrap();
        assert_eq!(cam.ptz_url(), "http://192.168.1.97:2020/onvif/ptz_service");
    }

    #[test]
    fn ptz_url_custom_onvif_port() {
        let mut config = make_tapo_config();
        config.onvif_port = Some(3000);
        let cam = TapoCamera::new(&config, None).unwrap();
        assert_eq!(cam.ptz_url(), "http://192.168.1.97:3000/onvif/ptz_service");
    }

    #[test]
    fn empty_credentials_default() {
        let mut config = make_tapo_config();
        config.username = None;
        config.password = None;
        let cam = TapoCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://:@192.168.1.97:554/stream1"
        );
    }
}
