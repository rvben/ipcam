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
    let created = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

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

/// Try to extract "manufacturer model" from an ONVIF GetDeviceInformation response.
fn parse_device_info(xml: &str) -> Option<String> {
    let model = extract_xml_elements(xml, "Model").into_iter().next()?;
    let mfr = extract_xml_elements(xml, "Manufacturer")
        .into_iter()
        .next()
        .unwrap_or_default();
    if mfr.is_empty() {
        Some(model)
    } else {
        Some(format!("{} {}", mfr, model))
    }
}

/// Query model name via ONVIF GetDeviceInformation.
/// Tries authenticated first, then unauthenticated, then GetScopes as last resort.
pub async fn query_model_name(
    client: &reqwest::Client,
    device_service_url: &str,
    username: &str,
    password: &str,
) -> Option<String> {
    let get_device_info =
        r#"<tds:GetDeviceInformation xmlns:tds="http://www.onvif.org/ver10/device/wsdl"/>"#;

    // Try with digest auth first
    let auth_body = soap_envelope(username, password, get_device_info);
    if let Some(result) = send_and_parse_device_info(client, device_service_url, &auth_body).await {
        return Some(result);
    }

    // Fall back to unauthenticated GetDeviceInformation
    let unauth_body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl">
  <s:Body>
    {get_device_info}
  </s:Body>
</s:Envelope>"#
    );
    if let Some(result) = send_and_parse_device_info(client, device_service_url, &unauth_body).await
    {
        return Some(result);
    }

    // Last resort: GetScopes (try unauth then auth, model in scope URI)
    query_model_from_scopes(client, device_service_url, username, password).await
}

/// Try to extract model name from ONVIF GetScopes response.
/// Scope URIs often contain: onvif://www.onvif.org/name/<model>
/// or onvif://www.onvif.org/hardware/<model>
/// Tries unauthenticated first, then authenticated.
async fn query_model_from_scopes(
    client: &reqwest::Client,
    device_service_url: &str,
    username: &str,
    password: &str,
) -> Option<String> {
    let get_scopes = r#"<tds:GetScopes xmlns:tds="http://www.onvif.org/ver10/device/wsdl"/>"#;

    // Try unauthenticated first
    let unauth_body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl">
  <s:Body>
    {get_scopes}
  </s:Body>
</s:Envelope>"#
    );
    if let Some(result) = send_and_parse_scopes(client, device_service_url, &unauth_body).await {
        return Some(result);
    }

    // Try with auth
    let auth_body = soap_envelope(username, password, get_scopes);
    send_and_parse_scopes(client, device_service_url, &auth_body).await
}

async fn send_and_parse_scopes(client: &reqwest::Client, url: &str, body: &str) -> Option<String> {
    let resp = client
        .post(url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(body.to_string())
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let xml = resp.text().await.ok()?;
    parse_scopes(&xml)
}

fn parse_scopes(xml: &str) -> Option<String> {
    let scope_items = extract_xml_elements(xml, "ScopeItem");
    let mut name = None;
    let mut hardware = None;
    for item in &scope_items {
        if let Some(suffix) = item.strip_prefix("onvif://www.onvif.org/name/") {
            let decoded = url_decode(suffix);
            if !decoded.is_empty() {
                name = Some(decoded);
            }
        } else if let Some(suffix) = item.strip_prefix("onvif://www.onvif.org/hardware/") {
            let decoded = url_decode(suffix);
            if !decoded.is_empty() {
                hardware = Some(decoded);
            }
        }
    }
    hardware.or(name)
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().and_then(hex_val);
            let lo = chars.next().and_then(hex_val);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h << 4 | l) as char);
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Validate that bytes start with the JPEG SOI marker (0xFF 0xD8).
fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

async fn send_and_parse_device_info(
    client: &reqwest::Client,
    url: &str,
    body: &str,
) -> Option<String> {
    let resp = client
        .post(url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(body.to_string())
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let xml = resp.text().await.ok()?;
    parse_device_info(&xml)
}

/// Query the ONVIF snapshot URI via the media service GetSnapshotUri.
/// Returns the HTTP(S) URL of the snapshot endpoint, or None if unavailable.
pub async fn query_snapshot_uri(
    client: &reqwest::Client,
    host: &str,
    onvif_port: u16,
    username: &str,
    password: &str,
) -> Option<String> {
    let media_url = format!("http://{}:{}/onvif/media_service", host, onvif_port);
    let body = r#"<trt:GetSnapshotUri xmlns:trt="http://www.onvif.org/ver10/media/wsdl">
  <trt:ProfileToken>profile_1</trt:ProfileToken>
</trt:GetSnapshotUri>"#;

    let envelope = soap_envelope(username, password, body);
    let resp = client
        .post(&media_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(envelope)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    let xml = resp.text().await.ok()?;
    extract_xml_elements(&xml, "Uri").into_iter().next()
}

/// Fetch a snapshot image via ONVIF GetSnapshotUri.
/// Returns the JPEG bytes on success, or None if ONVIF snapshot is not available.
pub async fn fetch_onvif_snapshot(
    client: &reqwest::Client,
    host: &str,
    onvif_port: u16,
    username: &str,
    password: &str,
) -> Option<Vec<u8>> {
    let uri = query_snapshot_uri(client, host, onvif_port, username, password).await?;
    let resp = client
        .get(&uri)
        .basic_auth(username, Some(password))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if !is_jpeg(&bytes) {
        return None;
    }
    Some(bytes.to_vec())
}

/// Capture a snapshot: try ONVIF GetSnapshotUri first, fall back to native RTSP frame grab.
pub async fn snapshot_with_fallback(
    client: &reqwest::Client,
    name: &str,
    host: &str,
    onvif_port: u16,
    username: &str,
    password: &str,
    rtsp_url: &str,
) -> Result<crate::camera::Snapshot> {
    use crate::camera::{ImageFormat, Snapshot};

    // Try ONVIF GetSnapshotUri first (faster, single HTTP request)
    if let Some(data) = fetch_onvif_snapshot(client, host, onvif_port, username, password).await {
        return Ok(Snapshot {
            camera_name: name.to_string(),
            timestamp: chrono::Utc::now(),
            data,
            format: ImageFormat::Jpeg,
        });
    }

    // Fall back to native RTSP frame grab (retina + openh264)
    let data = crate::rtsp_grab::grab_frame(rtsp_url)
        .await
        .with_context(|| {
            format!(
                "snapshot failed for camera '{}' ({})",
                name,
                crate::redact_url(rtsp_url)
            )
        })?;

    Ok(Snapshot {
        camera_name: name.to_string(),
        timestamp: chrono::Utc::now(),
        data,
        format: ImageFormat::Jpeg,
    })
}

/// Check camera reachability via TCP connect to RTSP port, then try to get
/// the model name via ONVIF for a richer status detail.
///
/// `fallback_label` is shown when ONVIF model query fails (e.g. camera type name).
pub async fn check_reachable(
    client: &reqwest::Client,
    host: &str,
    rtsp_port: u16,
    device_service_url: &str,
    username: &str,
    password: &str,
    fallback_label: &str,
) -> HealthStatus {
    let start = Instant::now();
    let addr = format!("{}:{}", host, rtsp_port);
    match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(_)) => {
            let detail = query_model_name(client, device_service_url, username, password)
                .await
                .unwrap_or_else(|| fallback_label.to_string());
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
