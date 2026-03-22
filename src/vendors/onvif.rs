use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::camera::{
    Camera, CameraInfo, HealthStatus, MotionStatus, PtzDirection, Snapshot, StreamQuality,
};
use crate::config::{CameraConfig, Go2rtcConfig};
use crate::discovery::extract_xml_elements;

use super::soap;

pub struct OnvifCamera {
    name: String,
    host: String,
    rtsp_port: u16,
    onvif_port: u16,
    username: String,
    password: String,
    onvif_username: String,
    onvif_password: String,
    main_stream: String,
    sub_stream: String,
    go2rtc_stream: Option<String>,
    go2rtc: Option<Go2rtcConfig>,
    client: reqwest::Client,
}

impl OnvifCamera {
    pub fn new(config: &CameraConfig, go2rtc: Option<&Go2rtcConfig>) -> Result<Self> {
        let username = config.username.clone().unwrap_or_default();
        let password = config.password.clone().unwrap_or_default();
        let (onvif_username, onvif_password) = config.onvif_credentials();

        let main_stream = config
            .main_stream
            .clone()
            .unwrap_or_else(|| "onvif1".to_string());
        let sub_stream = config
            .sub_stream
            .clone()
            .unwrap_or_else(|| "onvif2".to_string());

        Ok(Self {
            name: config.name.clone(),
            host: config.host.clone(),
            rtsp_port: config.rtsp_port,
            onvif_port: config.onvif_port(),
            username,
            password,
            onvif_username,
            onvif_password,
            main_stream,
            sub_stream,
            go2rtc_stream: config.go2rtc_stream.clone(),
            go2rtc: go2rtc.cloned(),
            client: reqwest::Client::new(),
        })
    }

    fn device_service_url(&self) -> String {
        format!(
            "http://{}:{}/onvif/device_service",
            self.host, self.onvif_port
        )
    }

    fn ptz_url(&self) -> String {
        format!(
            "http://{}:{}/onvif/ptz_service",
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
            let path = match quality {
                StreamQuality::Main => &self.main_stream,
                StreamQuality::Sub => &self.sub_stream,
            };
            format!(
                "rtsp://{}:{}@{}:{}/{}",
                self.username, self.password, self.host, self.rtsp_port, path
            )
        }
    }
}

#[async_trait]
impl Camera for OnvifCamera {
    async fn info(&self) -> Result<CameraInfo> {
        let body = r#"<tds:GetDeviceInformation xmlns:tds="http://www.onvif.org/ver10/device/wsdl"/>"#;
        let envelope = soap::soap_envelope(&self.onvif_username, &self.onvif_password, body);

        let resp = self
            .client
            .post(self.device_service_url())
            .header("Content-Type", "application/soap+xml; charset=utf-8")
            .body(envelope)
            .send()
            .await
            .with_context(|| {
                format!(
                    "camera '{}' at {} is not reachable (ONVIF device service)",
                    self.name, self.host
                )
            })?;

        let xml = resp.text().await?;
        let model = extract_xml_elements(&xml, "Model").into_iter().next();
        let firmware = extract_xml_elements(&xml, "FirmwareVersion")
            .into_iter()
            .next();

        Ok(CameraInfo {
            name: self.name.clone(),
            model,
            firmware,
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
        bail!("motion detection is not supported for generic ONVIF cameras")
    }

    async fn is_reachable(&self) -> HealthStatus {
        soap::check_reachable(
            &self.client,
            &self.host,
            self.rtsp_port,
            &self.device_service_url(),
            &self.onvif_username,
            &self.onvif_password,
            "ONVIF",
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

    fn make_onvif_config() -> CameraConfig {
        CameraConfig {
            name: "garage".to_string(),
            camera_type: CameraType::Onvif,
            host: "192.168.1.50".to_string(),
            rtsp_port: 554,
            username: Some("admin".to_string()),
            password: Some("password".to_string()),
            go2rtc_stream: None,
            onvif_port: None,
            frigate_name: None,
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
    fn direct_rtsp_url_main_default_path() {
        let config = make_onvif_config();
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://admin:password@192.168.1.50:554/onvif1"
        );
    }

    #[test]
    fn direct_rtsp_url_sub_default_path() {
        let config = make_onvif_config();
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Sub),
            "rtsp://admin:password@192.168.1.50:554/onvif2"
        );
    }

    #[test]
    fn custom_stream_paths() {
        let mut config = make_onvif_config();
        config.main_stream = Some("Streaming/Channels/101".to_string());
        config.sub_stream = Some("Streaming/Channels/102".to_string());
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://admin:password@192.168.1.50:554/Streaming/Channels/101"
        );
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Sub),
            "rtsp://admin:password@192.168.1.50:554/Streaming/Channels/102"
        );
    }

    #[test]
    fn go2rtc_restream_url_main() {
        let mut config = make_onvif_config();
        config.go2rtc_stream = Some("garage".to_string());
        let go2rtc = make_go2rtc();
        let cam = OnvifCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://192.168.1.180:8554/garage"
        );
    }

    #[test]
    fn go2rtc_restream_url_sub() {
        let mut config = make_onvif_config();
        config.go2rtc_stream = Some("garage".to_string());
        let go2rtc = make_go2rtc();
        let cam = OnvifCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Sub),
            "rtsp://192.168.1.180:8554/garage_sub"
        );
    }

    #[test]
    fn no_go2rtc_stream_ignores_go2rtc_config() {
        let config = make_onvif_config();
        let go2rtc = make_go2rtc();
        let cam = OnvifCamera::new(&config, Some(&go2rtc)).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://admin:password@192.168.1.50:554/onvif1"
        );
    }

    #[test]
    fn device_service_url_default_port() {
        let config = make_onvif_config();
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.device_service_url(),
            "http://192.168.1.50:80/onvif/device_service"
        );
    }

    #[test]
    fn device_service_url_custom_port() {
        let mut config = make_onvif_config();
        config.onvif_port = Some(8080);
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.device_service_url(),
            "http://192.168.1.50:8080/onvif/device_service"
        );
    }

    #[test]
    fn ptz_url_uses_onvif_port() {
        let config = make_onvif_config();
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.ptz_url(),
            "http://192.168.1.50:80/onvif/ptz_service"
        );
    }

    #[test]
    fn empty_credentials_default() {
        let mut config = make_onvif_config();
        config.username = None;
        config.password = None;
        let cam = OnvifCamera::new(&config, None).unwrap();
        assert_eq!(
            cam.effective_rtsp_url(StreamQuality::Main),
            "rtsp://:@192.168.1.50:554/onvif1"
        );
    }
}
