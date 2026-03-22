use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::config::Config;

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
pub fn extract_xml_elements(xml: &str, local_name: &str) -> Vec<String> {
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

/// Parse ONVIF scope URIs from a ProbeMatch response to extract manufacturer and model.
///
/// Cameras advertise metadata in their Scopes element using URIs like:
/// - `onvif://www.onvif.org/name/CameraName`
/// - `onvif://www.onvif.org/hardware/ModelName`
/// - `onvif://www.onvif.org/manufacturer/Vendor`
/// - `onvif://www.onvif.org/Profile/Streaming`
///
/// Returns (manufacturer, model) where either may be None.
fn extract_scopes_info(xml: &str) -> (Option<String>, Option<String>) {
    let scopes = extract_xml_elements(xml, "Scopes");
    let mut manufacturer = None;
    let mut model = None;

    for scope_line in &scopes {
        for uri in scope_line.split_whitespace() {
            // Decode percent-encoded characters (e.g. %20 -> space)
            let decoded = percent_decode(uri);

            if let Some(value) = decoded.strip_prefix("onvif://www.onvif.org/name/") {
                let value = value.trim();
                if !value.is_empty() && manufacturer.is_none() {
                    manufacturer = Some(value.to_string());
                }
            } else if let Some(value) = decoded.strip_prefix("onvif://www.onvif.org/manufacturer/")
            {
                let value = value.trim();
                if !value.is_empty() {
                    // Prefer explicit manufacturer over name
                    manufacturer = Some(value.to_string());
                }
            } else if let Some(value) = decoded.strip_prefix("onvif://www.onvif.org/hardware/") {
                let value = value.trim();
                if !value.is_empty() && model.is_none() {
                    model = Some(value.to_string());
                }
            }
        }
    }

    (manufacturer, model)
}

/// Simple percent-decoding for ONVIF scope URIs.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                let hex = [h, l];
                if let Ok(s) = std::str::from_utf8(&hex)
                    && let Ok(byte) = u8::from_str_radix(s, 16)
                {
                    result.push(byte as char);
                    continue;
                }
                // Failed to decode, emit as-is
                result.push('%');
                result.push(h as char);
                result.push(l as char);
            }
        } else {
            result.push(b as char);
        }
    }
    result
}

/// Parse the IP address out of an ONVIF service URL.
fn address_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// List all local IPv4 addresses by parsing `ifconfig` output.
fn local_ipv4_addresses() -> Vec<Ipv4Addr> {
    let output = std::process::Command::new("ifconfig")
        .output()
        .ok()
        .filter(|o| o.status.success());

    let Some(output) = output else {
        return vec![Ipv4Addr::UNSPECIFIED];
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut addrs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("inet ")
            && let Some(ip_str) = rest.split_whitespace().next()
            && let Ok(ip) = ip_str.parse::<Ipv4Addr>()
            && !ip.is_loopback()
        {
            addrs.push(ip);
        }
    }

    if addrs.is_empty() {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        addrs
    }
}

/// Send a WS-Discovery Probe over UDP multicast and collect raw XML responses.
/// Sends on all local interfaces to ensure cameras on any subnet are reached.
fn send_probe(timeout: Duration) -> Result<Vec<String>> {
    let socket = UdpSocket::bind(LOCAL_BIND_ADDR).context("bind UDP socket")?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .context("set read timeout")?;

    let multicast_addr: Ipv4Addr = "239.255.255.250".parse().unwrap();
    let local_ips = local_ipv4_addresses();

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

    // Send probe on each local interface so cameras on any subnet are reached.
    for ip in &local_ips {
        if let Err(e) = socket.join_multicast_v4(&multicast_addr, ip) {
            tracing::debug!("join multicast on {}: {}", ip, e);
        }
        // Set outgoing interface for this probe.
        let octets = ip.octets();
        // Set outgoing multicast interface via setsockopt IP_MULTICAST_IF.
        use std::os::unix::io::AsRawFd;
        let fd = socket.as_raw_fd();
        let result = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_IP,
                libc::IP_MULTICAST_IF,
                octets.as_ptr() as *const libc::c_void,
                4,
            )
        };
        if result != 0 {
            tracing::debug!(
                "set multicast IF {}: errno {}",
                ip,
                std::io::Error::last_os_error()
            );
            continue;
        }
        tracing::debug!(
            "sending probe via {}.{}.{}.{}",
            octets[0],
            octets[1],
            octets[2],
            octets[3]
        );
        if let Err(e) = socket.send_to(probe.as_bytes(), dest) {
            tracing::debug!("send probe via {}: {}", ip, e);
        }
    }

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

/// Build a SOAP envelope with WS-Security Password Digest authentication.
///
/// Uses the same digest scheme as the ONVIF spec:
/// Digest = Base64(SHA1(nonce + created + password))
fn get_device_info_request_digest(username: &str, password: &str) -> String {
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
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl"
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
    <tds:GetDeviceInformation/>
  </s:Body>
</s:Envelope>"#,
        nonce = nonce_b64,
    )
}

/// Build the SOAP envelope for an unauthenticated ONVIF GetDeviceInformation request.
fn get_device_info_request_unauth() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl">
  <s:Body>
    <tds:GetDeviceInformation/>
  </s:Body>
</s:Envelope>"#
        .to_string()
}

/// Send a GetDeviceInformation SOAP request and parse (manufacturer, model) from the response.
async fn send_device_info_request(
    client: &reqwest::Client,
    onvif_url: &str,
    body: &str,
) -> Option<(String, String)> {
    let resp = client
        .post(onvif_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(body.to_string())
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;

    let xml = resp.text().await.ok()?;

    let manufacturer = extract_xml_elements(&xml, "Manufacturer")
        .into_iter()
        .next()?;
    let model = extract_xml_elements(&xml, "Model").into_iter().next()?;

    Some((manufacturer, model))
}

/// Call ONVIF GetDeviceInformation on a device and return (manufacturer, model).
///
/// Tries unauthenticated first. If that fails and credentials are provided,
/// retries with WS-Security Password Digest authentication.
async fn get_device_info(
    client: &reqwest::Client,
    onvif_url: &str,
    credentials: Option<(&str, &str)>,
) -> Option<(String, String)> {
    // Try unauthenticated first
    let unauth_body = get_device_info_request_unauth();
    if let Some(result) = send_device_info_request(client, onvif_url, &unauth_body).await {
        return Some(result);
    }

    // If we have credentials, retry with digest auth
    let (username, password) = credentials?;
    tracing::debug!(
        "unauthenticated GetDeviceInformation failed for {}, retrying with digest auth",
        onvif_url
    );
    let digest_body = get_device_info_request_digest(username, password);
    send_device_info_request(client, onvif_url, &digest_body).await
}

/// Check if a response body looks like ONVIF SOAP XML.
/// Checks for SOAP namespaces or ONVIF-specific content.
fn looks_like_onvif_soap(body: &str) -> bool {
    // SOAP 1.1 namespace
    body.contains("schemas.xmlsoap.org")
        // SOAP 1.2 namespace
        || body.contains("www.w3.org/2003/05/soap-envelope")
        // ONVIF namespace
        || body.contains("www.onvif.org")
}

/// Probe a single ONVIF endpoint: verify it's actually ONVIF, and try to get
/// device info. Combines the "is it ONVIF?" check with the device info query
/// in a single request to avoid redundant HTTP calls.
///
/// Returns `Some(DiscoveredCamera)` if the host is an ONVIF device,
/// `None` if it's not ONVIF or unreachable.
async fn probe_onvif_endpoint(
    client: &reqwest::Client,
    address: String,
    onvif_url: String,
    credentials: Option<(String, String)>,
) -> Option<DiscoveredCamera> {
    let unauth_body = get_device_info_request_unauth();
    let resp = client
        .post(&onvif_url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(unauth_body)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;

    // Check Content-Type first — ONVIF services return XML/SOAP content types
    let is_xml_response = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("xml") || ct.contains("soap"));

    let xml = resp.text().await.ok()?;

    // Verify this is a real ONVIF response, not an echo of our request.
    // Some web servers echo the POST body in error pages, which would contain
    // our SOAP namespaces. A genuine ONVIF response will have XML content-type
    // AND contain response-specific elements (Fault, Body with response data).
    let is_onvif = if is_xml_response {
        looks_like_onvif_soap(&xml)
    } else {
        // No XML content-type: only accept if we see ONVIF-response-specific content
        // that wouldn't appear in our echoed request
        xml.contains("Fault") || xml.contains("GetDeviceInformationResponse")
    };

    if !is_onvif {
        tracing::debug!("{} is not an ONVIF service", onvif_url);
        return None;
    }

    // Try to extract device info from the unauthenticated response
    let manufacturer = extract_xml_elements(&xml, "Manufacturer").into_iter().next();
    let model = extract_xml_elements(&xml, "Model").into_iter().next();

    if manufacturer.is_some() && model.is_some() {
        return Some(DiscoveredCamera {
            address,
            onvif_url,
            manufacturer,
            model,
            types: vec![],
        });
    }

    // Unauthenticated didn't return device info (likely needs auth). Try with credentials.
    if let Some((username, password)) = credentials {
        tracing::debug!("retrying {} with digest auth", onvif_url);
        let digest_body = get_device_info_request_digest(&username, &password);
        if let Some((mfr, mdl)) = send_device_info_request(client, &onvif_url, &digest_body).await
        {
            return Some(DiscoveredCamera {
                address,
                onvif_url,
                manufacturer: Some(mfr),
                model: Some(mdl),
                types: vec![],
            });
        }
    }

    // It's ONVIF but we couldn't get device info (auth required, no creds available)
    Some(DiscoveredCamera {
        address,
        onvif_url,
        manufacturer: None,
        model: None,
        types: vec![],
    })
}

/// Discover cameras on the local network via WS-Discovery.
///
/// Sends a UDP multicast Probe and waits for ProbeMatch responses for `timeout`
/// duration. For each unique ONVIF endpoint found, attempts to fetch basic
/// device information.
///
/// When `config` is provided, credentials from matching camera entries are used
/// for authenticated GetDeviceInformation requests when unauthenticated requests
/// fail. Manufacturer and model are also extracted from WS-Discovery Scopes
/// as a fallback.
pub async fn discover_cameras(
    timeout: Duration,
    config: Option<&Config>,
) -> Result<Vec<DiscoveredCamera>> {
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
        let (scope_manufacturer, scope_model) = extract_scopes_info(xml);

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

            // Look up credentials from config if available
            let credentials = config
                .and_then(|cfg| {
                    cfg.cameras
                        .iter()
                        .find(|c| c.host == address)
                })
                .and_then(|cam| {
                    match (cam.username.as_deref(), cam.password.as_deref()) {
                        (Some(u), Some(p)) => Some((u, p)),
                        _ => None,
                    }
                });

            let (manufacturer, model) =
                match get_device_info(&client, &onvif_url, credentials).await {
                    Some((mfr, mdl)) => (Some(mfr), Some(mdl)),
                    None => {
                        tracing::debug!("no device info from {}, using scopes", onvif_url);
                        (scope_manufacturer.clone(), scope_model.clone())
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

/// Common ONVIF ports to probe when scanning a subnet.
const ONVIF_PORTS: &[u16] = &[2020, 8000, 80, 8080];

/// Parse a CIDR notation string (e.g. "10.10.20.0/24") into a list of host IPs.
/// Excludes the network and broadcast addresses.
fn parse_cidr(cidr: &str) -> Result<Vec<Ipv4Addr>> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        bail!("invalid CIDR notation: expected format like 10.10.20.0/24");
    }

    let base_ip: Ipv4Addr = parts[0].parse().context("invalid IP address in CIDR")?;
    let prefix_len: u32 = parts[1].parse().context("invalid prefix length in CIDR")?;

    if prefix_len > 32 {
        bail!("prefix length must be 0-32, got {}", prefix_len);
    }
    if prefix_len < 16 {
        bail!(
            "prefix /{} is too broad ({} hosts). Use /16 or narrower.",
            prefix_len,
            2u32.pow(32 - prefix_len) - 2
        );
    }

    let base: u32 = u32::from(base_ip);
    let mask: u32 = if prefix_len == 32 {
        u32::MAX
    } else {
        !((1u32 << (32 - prefix_len)) - 1)
    };
    let network = base & mask;
    let host_count = 1u32 << (32 - prefix_len);

    if host_count <= 2 {
        // /31 or /32: return just the IP(s)
        return Ok(vec![base_ip]);
    }

    // Skip network address (first) and broadcast (last)
    let addrs = (1..host_count - 1)
        .map(|i| Ipv4Addr::from(network + i))
        .collect();

    Ok(addrs)
}

/// Probe a single IP on multiple ONVIF ports via TCP connect.
/// Returns the first port that accepts a connection, or None.
async fn probe_onvif_ports(ip: Ipv4Addr, timeout: Duration) -> Option<(Ipv4Addr, u16)> {
    for &port in ONVIF_PORTS {
        let addr = SocketAddr::from((ip, port));
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {
                tracing::debug!("ONVIF port open: {}:{}", ip, port);
                return Some((ip, port));
            }
            _ => continue,
        }
    }
    None
}

/// Scan a subnet for cameras by TCP-probing common ONVIF ports, then querying
/// responsive hosts for device information.
///
/// Use this for cameras on remote VLANs that multicast WS-Discovery can't reach.
pub async fn scan_subnet(
    cidr: &str,
    timeout: Duration,
    config: Option<&Config>,
) -> Result<Vec<DiscoveredCamera>> {
    let ips = parse_cidr(cidr)?;
    tracing::info!("scanning {} hosts in {}", ips.len(), cidr);

    // Probe all IPs in parallel with a short per-host TCP timeout.
    // Use the smaller of 500ms or the user-provided timeout for TCP probes.
    let probe_timeout = timeout.min(Duration::from_millis(500));
    let mut handles = Vec::with_capacity(ips.len());
    for ip in ips {
        handles.push(tokio::spawn(async move {
            probe_onvif_ports(ip, probe_timeout).await
        }));
    }

    let mut responsive = Vec::new();
    for handle in handles {
        if let Ok(Some((ip, port))) = handle.await {
            responsive.push((ip, port));
        }
    }

    tracing::info!("found {} responsive host(s)", responsive.len());

    // Query all responsive hosts in parallel for ONVIF verification + device info
    let client = reqwest::Client::new();
    let mut seen = HashSet::new();
    let mut onvif_handles = Vec::new();

    for (ip, port) in responsive {
        let addr = ip.to_string();
        if !seen.insert(addr.clone()) {
            continue;
        }

        let onvif_url = format!("http://{}:{}/onvif/device_service", ip, port);

        let credentials = config
            .and_then(|cfg| {
                cfg.cameras
                    .iter()
                    .find(|c| c.host == addr)
                    .and_then(|cam| match (cam.username.as_deref(), cam.password.as_deref()) {
                        (Some(u), Some(p)) => Some((u.to_string(), p.to_string())),
                        _ => None,
                    })
            });

        let client = client.clone();
        onvif_handles.push(tokio::spawn(async move {
            probe_onvif_endpoint(&client, addr, onvif_url, credentials).await
        }));
    }

    let mut cameras = Vec::new();
    for handle in onvif_handles {
        if let Ok(Some(cam)) = handle.await {
            cameras.push(cam);
        }
    }

    Ok(cameras)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_scopes_manufacturer_and_model() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery">
  <s:Body>
    <d:ProbeMatches>
      <d:ProbeMatch>
        <d:Scopes>onvif://www.onvif.org/name/Reolink onvif://www.onvif.org/hardware/D340W onvif://www.onvif.org/Profile/Streaming</d:Scopes>
      </d:ProbeMatch>
    </d:ProbeMatches>
  </s:Body>
</s:Envelope>"#;
        let (mfr, model) = extract_scopes_info(xml);
        assert_eq!(mfr.as_deref(), Some("Reolink"));
        assert_eq!(model.as_deref(), Some("D340W"));
    }

    #[test]
    fn extract_scopes_manufacturer_uri_preferred() {
        let xml = r#"<Envelope><Body><ProbeMatches><ProbeMatch>
        <Scopes>onvif://www.onvif.org/name/GenericCam onvif://www.onvif.org/manufacturer/ACME onvif://www.onvif.org/hardware/Model99</Scopes>
        </ProbeMatch></ProbeMatches></Body></Envelope>"#;
        let (mfr, model) = extract_scopes_info(xml);
        // manufacturer/ URI takes precedence over name/
        assert_eq!(mfr.as_deref(), Some("ACME"));
        assert_eq!(model.as_deref(), Some("Model99"));
    }

    #[test]
    fn extract_scopes_with_percent_encoding() {
        let xml = r#"<Envelope><Body><ProbeMatches><ProbeMatch>
        <Scopes>onvif://www.onvif.org/name/TP-Link%20Camera onvif://www.onvif.org/hardware/Tapo%20C200</Scopes>
        </ProbeMatch></ProbeMatches></Body></Envelope>"#;
        let (mfr, model) = extract_scopes_info(xml);
        assert_eq!(mfr.as_deref(), Some("TP-Link Camera"));
        assert_eq!(model.as_deref(), Some("Tapo C200"));
    }

    #[test]
    fn extract_scopes_empty() {
        let xml = r#"<Envelope><Body><ProbeMatches><ProbeMatch>
        <Scopes>onvif://www.onvif.org/Profile/Streaming</Scopes>
        </ProbeMatch></ProbeMatches></Body></Envelope>"#;
        let (mfr, model) = extract_scopes_info(xml);
        assert!(mfr.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn extract_scopes_no_scopes_element() {
        let xml = r#"<Envelope><Body><ProbeMatches><ProbeMatch>
        <XAddrs>http://192.168.1.1/onvif/device_service</XAddrs>
        </ProbeMatch></ProbeMatches></Body></Envelope>"#;
        let (mfr, model) = extract_scopes_info(xml);
        assert!(mfr.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("no%20encoding%21"), "no encoding!");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn percent_decode_invalid_sequence() {
        // Invalid hex should be passed through
        assert_eq!(percent_decode("bad%ZZvalue"), "bad%ZZvalue");
    }

    #[test]
    fn digest_auth_request_contains_security_header() {
        let body = get_device_info_request_digest("admin", "password123");
        assert!(body.contains("<wsse:Username>admin</wsse:Username>"));
        assert!(body.contains("PasswordDigest"));
        assert!(body.contains("<wsu:Created>"));
        assert!(body.contains("<wsse:Nonce"));
        assert!(body.contains("GetDeviceInformation"));
    }

    #[test]
    fn unauth_request_has_no_security_header() {
        let body = get_device_info_request_unauth();
        assert!(!body.contains("wsse"));
        assert!(!body.contains("Security"));
        assert!(body.contains("GetDeviceInformation"));
    }

    #[test]
    fn looks_like_onvif_soap_detects_real_responses() {
        // ONVIF auth failure response
        assert!(looks_like_onvif_soap(
            r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><s:Fault>NotAuthorized</s:Fault></s:Body></s:Envelope>"#
        ));
        // ONVIF success response
        assert!(looks_like_onvif_soap(
            r#"<tds:GetDeviceInformationResponse xmlns:tds="http://www.onvif.org/ver10/device/wsdl"></tds:GetDeviceInformationResponse>"#
        ));
    }

    #[test]
    fn looks_like_onvif_soap_rejects_non_onvif() {
        assert!(!looks_like_onvif_soap("<html><body>Hello</body></html>"));
        assert!(!looks_like_onvif_soap("404 Not Found"));
        assert!(!looks_like_onvif_soap(""));
    }

    #[test]
    fn parse_cidr_24() {
        let ips = parse_cidr("10.10.20.0/24").unwrap();
        assert_eq!(ips.len(), 254);
        assert_eq!(ips[0], Ipv4Addr::new(10, 10, 20, 1));
        assert_eq!(ips[253], Ipv4Addr::new(10, 10, 20, 254));
    }

    #[test]
    fn parse_cidr_28() {
        let ips = parse_cidr("192.168.1.0/28").unwrap();
        assert_eq!(ips.len(), 14);
        assert_eq!(ips[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(ips[13], Ipv4Addr::new(192, 168, 1, 14));
    }

    #[test]
    fn parse_cidr_32() {
        let ips = parse_cidr("10.10.20.5/32").unwrap();
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0], Ipv4Addr::new(10, 10, 20, 5));
    }

    #[test]
    fn parse_cidr_rejects_too_broad() {
        assert!(parse_cidr("10.0.0.0/8").is_err());
    }

    #[test]
    fn parse_cidr_rejects_invalid() {
        assert!(parse_cidr("not-a-cidr").is_err());
        assert!(parse_cidr("10.10.20.0/33").is_err());
        assert!(parse_cidr("10.10.20.0").is_err());
    }
}
