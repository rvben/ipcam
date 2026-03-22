use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use base64::Engine;
use sha1::{Digest, Sha1};

use crate::camera::HealthStatus;
use crate::discovery::extract_xml_elements;

/// Build a SOAP envelope with WS-Security UsernameToken using Password Digest.
/// Digest = Base64(SHA1(nonce + created + password))
pub fn soap_envelope(username: &str, password: &str, body: &str) -> String {
    let nonce_bytes: [u8; 16] = rand::random();
    let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);
    let created = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let mut hasher = Sha1::new();
    hasher.update(nonce_bytes);
    hasher.update(created.as_bytes());
    hasher.update(password.as_bytes());
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
        nonce = nonce_b64,
    )
}

/// Send a SOAP request to an ONVIF PTZ endpoint.
pub async fn send_ptz_soap(
    client: &reqwest::Client,
    ptz_url: &str,
    username: &str,
    password: &str,
    camera_name: &str,
    host: &str,
    body: &str,
) -> Result<()> {
    let envelope = soap_envelope(username, password, body);
    let resp = client
        .post(ptz_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(envelope)
        .send()
        .await
        .with_context(|| {
            format!(
                "camera '{}' at {} is not reachable (ONVIF PTZ service)",
                camera_name, host
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("ONVIF PTZ request failed ({}): {}", status, text);
    }
    Ok(())
}

use anyhow::Context;

/// Query model name via ONVIF GetDeviceInformation (with digest auth).
/// Returns "manufacturer model" string, or None on failure.
pub async fn query_model_name(
    client: &reqwest::Client,
    device_service_url: &str,
    username: &str,
    password: &str,
) -> Option<String> {
    let body = soap_envelope(
        username,
        password,
        r#"<tds:GetDeviceInformation xmlns:tds="http://www.onvif.org/ver10/device/wsdl"/>"#,
    );
    let resp = client
        .post(device_service_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(body)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let xml = resp.text().await.ok()?;
    let model = extract_xml_elements(&xml, "Model").into_iter().next()?;
    let mfr = extract_xml_elements(&xml, "Manufacturer")
        .into_iter()
        .next()
        .unwrap_or_default();
    if mfr.is_empty() {
        Some(model)
    } else {
        Some(format!("{} {}", mfr, model))
    }
}

/// Check camera reachability via TCP connect to RTSP port, then try to get
/// the model name via ONVIF for a richer status detail.
pub async fn check_reachable(
    client: &reqwest::Client,
    host: &str,
    rtsp_port: u16,
    device_service_url: &str,
    username: &str,
    password: &str,
) -> HealthStatus {
    let start = Instant::now();
    let addr = format!("{}:{}", host, rtsp_port);
    match tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(&addr)).await
    {
        Ok(Ok(_)) => {
            let detail = query_model_name(client, device_service_url, username, password)
                .await
                .unwrap_or(addr);
            HealthStatus {
                online: true,
                detail,
                latency: start.elapsed(),
            }
        }
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
