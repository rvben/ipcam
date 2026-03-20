use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::FrigateConfig;

pub struct FrigateClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrigateEvent {
    pub id: String,
    pub camera: String,
    pub label: String,
    pub score: Option<f64>,
    pub start_time: f64,
    pub end_time: Option<f64>,
    pub thumbnail: Option<String>,
}

impl FrigateClient {
    pub fn new(config: &FrigateConfig) -> Self {
        Self {
            base_url: config.base_url(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn events(&self, camera: Option<&str>, limit: u32) -> Result<Vec<FrigateEvent>> {
        let mut url = format!("{}/api/events?limit={}", self.base_url, limit);
        if let Some(cam) = camera {
            url.push_str(&format!("&camera={}", cam));
        }

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("connecting to Frigate")?;

        if !resp.status().is_success() {
            bail!("Frigate API returned status {} for {}", resp.status(), url,);
        }

        resp.json().await.context("parsing Frigate events response")
    }

    pub async fn snapshot(&self, camera: &str, output: Option<PathBuf>) -> Result<PathBuf> {
        let url = format!("{}/api/{}/latest.jpg", self.base_url, camera);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("connecting to Frigate")?;

        if !resp.status().is_success() {
            bail!("Frigate API returned status {} for {}", resp.status(), url,);
        }

        let data = resp.bytes().await?;

        let path = output.unwrap_or_else(|| {
            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            PathBuf::from(format!("{}_{}_frigate.jpg", camera, ts))
        });

        std::fs::write(&path, &data)
            .with_context(|| format!("writing snapshot to {}", path.display()))?;

        Ok(path)
    }
}
