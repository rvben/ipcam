use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

const WS_DISCOVERY_ADDR: &str = "239.255.255.250:3702";
const LOCAL_BIND_ADDR: &str = "0.0.0.0:0";

/// A camera found via WS-Discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredCamera {
    pub address: String,
    pub onvif_url: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub types: Vec<String>,
}

/// Build a WS-Discovery Probe SOAP message with a unique message ID.
fn build_probe_message(uuid: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing"
            xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"
            xmlns:dn="http://www.onvif.org/ver10/network/wsdl">
  <s:Header>
    <a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</a:Action>
    <a:MessageID>uuid:{uuid}</a:MessageID>
    <a:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</a:To>
  </s:Header>
  <s:Body>
    <d:Probe>
      <d:Types>dn:NetworkVideoTransmitter</d:Types>
    </d:Probe>
  </s:Body>
</s:Envelope>"#
    )
}

/// Extract text content of a named XML element from a response string.
/// Returns all occurrences.
fn extract_xml_elements(xml: &str, local_name: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut results = Vec::new();
    let mut inside = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.local_name();
                if name.as_ref() == local_name.as_bytes() {
                    inside = true;
                }
            }
            Ok(Event::Text(e)) if inside => {
                if let Ok(text) = e.unescape() {
                    let s = text.trim().to_string();
                    if !s.is_empty() {
                        results.push(s);
                    }
                }
                inside = false;
            }
            Ok(Event::End(_)) => {
                inside = false;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    results
}

/// Extract all XAddrs (ONVIF endpoint URLs) from a ProbeMatch response.
fn extract_xaddrs(xml: &str) -> Vec<String> {
    extract_xml_elements(xml, "XAddrs")
        .into_iter()
        .flat_map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// Extract Types from a ProbeMatch response (e.g. "dn:NetworkVideoTransmitter").
fn extract_types(xml: &str) -> Vec<String> {
    extract_xml_elements(xml, "Types")
        .into_iter()
        .flat_map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// Parse the IP address out of an ONVIF service URL.
fn address_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// Send a WS-Discovery Probe over UDP multicast and collect raw XML responses.
fn send_probe(timeout: Duration) -> Result<Vec<String>> {
    let socket = UdpSocket::bind(LOCAL_BIND_ADDR).context("bind UDP socket")?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .context("set read timeout")?;

    // Join the multicast group so we can receive responses.
    let multicast_addr: Ipv4Addr = "239.255.255.250".parse().unwrap();
    socket
        .join_multicast_v4(&multicast_addr, &Ipv4Addr::UNSPECIFIED)
        .context("join multicast group")?;

    // Generate a UUID-like message ID from the current time.
    let msg_id = format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        0x4a2b_u32,
        0x11ee_u32,
        0x8080_u32,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos(),
    );

    let probe = build_probe_message(&msg_id);
    let dest: SocketAddr = WS_DISCOVERY_ADDR.parse().unwrap();
    socket
        .send_to(probe.as_bytes(), dest)
        .context("send Probe")?;

    let deadline = std::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 65535];
    let mut responses = Vec::new();

    while std::time::Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, _src)) => {
                if let Ok(xml) = std::str::from_utf8(&buf[..len]) {
                    responses.push(xml.to_string());
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // No data yet; keep polling until deadline.
            }
            Err(e) => {
                tracing::warn!("UDP recv error: {}", e);
                break;
            }
        }
    }

    Ok(responses)
}

/// Build the SOAP envelope for ONVIF GetDeviceInformation.
fn get_device_info_request(username: Option<&str>, password: Option<&str>) -> String {
    let security_header = match (username, password) {
        (Some(u), Some(p)) => format!(
            r#"<s:Header>
    <Security xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd">
      <UsernameToken>
        <Username>{u}</Username>
        <Password>{p}</Password>
      </UsernameToken>
    </Security>
  </s:Header>"#
        ),
        _ => String::new(),
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl">
  {security_header}
  <s:Body>
    <tds:GetDeviceInformation/>
  </s:Body>
</s:Envelope>"#
    )
}

/// Call ONVIF GetDeviceInformation on a device and return (manufacturer, model).
async fn get_device_info(client: &reqwest::Client, onvif_url: &str) -> Option<(String, String)> {
    let body = get_device_info_request(None, None);

    let resp = client
        .post(onvif_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .header(
            "SOAPAction",
            "http://www.onvif.org/ver10/device/wsdl/GetDeviceInformation",
        )
        .body(body)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;

    let xml = resp.text().await.ok()?;

    let manufacturers = extract_xml_elements(&xml, "Manufacturer");
    let models = extract_xml_elements(&xml, "Model");

    let manufacturer = manufacturers.into_iter().next()?;
    let model = models.into_iter().next()?;

    Some((manufacturer, model))
}

/// Discover cameras on the local network via WS-Discovery.
///
/// Sends a UDP multicast Probe and waits for ProbeMatch responses for `timeout`
/// duration. For each unique ONVIF endpoint found, attempts to fetch basic
/// device information.
pub async fn discover_cameras(timeout: Duration) -> Result<Vec<DiscoveredCamera>> {
    tracing::info!(
        "sending WS-Discovery probe (timeout: {}s)...",
        timeout.as_secs()
    );

    let responses = tokio::task::spawn_blocking(move || send_probe(timeout))
        .await
        .context("discovery task panicked")?
        .context("UDP probe failed")?;

    tracing::info!("received {} probe response(s)", responses.len());

    let client = reqwest::Client::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut cameras = Vec::new();

    for xml in &responses {
        let xaddrs = extract_xaddrs(xml);
        let types = extract_types(xml);

        for xaddr in xaddrs {
            // Normalise to the device service endpoint.
            let onvif_url = if xaddr.contains("onvif") || xaddr.contains("device") {
                xaddr.clone()
            } else {
                format!("{}/onvif/device_service", xaddr.trim_end_matches('/'))
            };

            if !seen_urls.insert(onvif_url.clone()) {
                continue;
            }

            let address = address_from_url(&xaddr);

            let (manufacturer, model) = match get_device_info(&client, &onvif_url).await {
                Some((mfr, mdl)) => (Some(mfr), Some(mdl)),
                None => {
                    tracing::debug!("no device info from {}", onvif_url);
                    (None, None)
                }
            };

            cameras.push(DiscoveredCamera {
                address,
                onvif_url,
                manufacturer,
                model,
                types: types.clone(),
            });
        }
    }

    Ok(cameras)
}
