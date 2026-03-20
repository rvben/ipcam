pub mod reolink;
pub mod tapo;

use anyhow::Result;

use crate::camera::Camera;
use crate::config::{CameraConfig, CameraType, Go2rtcConfig};

pub fn create_camera(
    config: &CameraConfig,
    go2rtc: Option<&Go2rtcConfig>,
) -> Result<Box<dyn Camera>> {
    match config.camera_type {
        CameraType::Tapo => Ok(Box::new(tapo::TapoCamera::new(config, go2rtc)?)),
        CameraType::Reolink => Ok(Box::new(reolink::ReolinkCamera::new(config)?)),
    }
}
