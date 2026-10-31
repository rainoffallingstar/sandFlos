use crate::models::{DashboardDevice, ShareTransferDevice};
use crate::util::{json_field_to_string, parse_bool_field, value_to_array};
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpStream, UdpSocket},
    process::{Command, Output},
    thread,
    time::Duration,
};
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use uuid::Uuid;

const SHARE_WS_WAIT_MS: u64 = 2_200;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn set_socket_read_timeout(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, timeout_ms: u64) {
    let timeout = Some(Duration::from_millis(timeout_ms));
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            let _ = stream.set_read_timeout(timeout);
        }
        MaybeTlsStream::Rustls(stream) => {
            let _ = stream.get_mut().set_read_timeout(timeout);
        }
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

pub fn fetch_share_devices(uid: &str) -> Vec<ShareTransferDevice> {
    let ws_id = Uuid::new_v4().to_string();
    let url = format!(
        "wss://send2boox.com/share/{}?type=client&id={}",
        urlencoding::encode(uid),
        urlencoding::encode(&ws_id)
    );

    let (mut socket, _) = match connect(url) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    set_socket_read_timeout(&mut socket, SHARE_WS_WAIT_MS);

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    while let Ok(message) = socket.read() {
        let Message::Text(text) = message else {
            continue;
        };
        let payload: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source_items = if action == "serverInfo"
            && payload.get("message").and_then(Value::as_str) == Some("yes")
        {
            value_to_array(payload.get("data").cloned().unwrap_or(Value::Null))
        } else if action == "serverStatus" {
            value_to_array(
                payload
                    .get("data")
                    .and_then(|value| value.get("serverInfo"))
                    .cloned()
                    .unwrap_or(Value::Null),
            )
        } else if action == "serverInfo" {
            Vec::new()
        } else {
            continue;
        };

        for item in source_items {
            let host = json_field_to_string(&item, "host")
                .and_then(|value| normalize_transfer_host_url(&value));
            let mac = json_field_to_string(&item, "mac")
                .or_else(|| json_field_to_string(&item, "macAddress"));
            let model = json_field_to_string(&item, "model");
            let status = json_field_to_string(&item, "status")
                .or_else(|| json_field_to_string(&item, "loginStatus"));
            let key = format!(
                "{}|{}|{}",
                mac.clone().unwrap_or_default(),
                host.clone().unwrap_or_default(),
                model.clone().unwrap_or_default()
            );
            if seen.insert(key) {
                items.push(ShareTransferDevice {
                    model,
                    mac_address: mac,
                    host,
                    status,
                });
            }
        }

        if !items.is_empty() || action == "serverInfo" {
            break;
        }
    }

    items
}

pub fn is_online_status(status: Option<&str>) -> bool {
    matches!(
        status
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("online") | Some("on")
    )
}

pub fn build_dashboard_devices(
    raw: Value,
    share_devices: Vec<ShareTransferDevice>,
) -> Vec<DashboardDevice> {
    let local_ipv4_candidates = collect_local_ipv4_candidates();
    let mut local_probe_ips = local_ipv4_candidates
        .iter()
        .map(|(_, ip)| *ip)
        .collect::<Vec<_>>();
    if local_probe_ips.is_empty() {
        if let Some(fallback) = detect_local_ipv4() {
            local_probe_ips.push(fallback);
        }
    }

    let mut neighbors = collect_arp_neighbors();
    let target_macs = value_to_array(raw.clone())
        .iter()
        .filter_map(|item| {
            json_field_to_string(item, "macAddress").or_else(|| json_field_to_string(item, "mac"))
        })
        .collect::<Vec<_>>();
    if !target_macs.is_empty()
        && !local_probe_ips.is_empty()
        && !has_any_mac_match(&neighbors, &target_macs)
    {
        warmup_arp_by_udp_sweep(&local_probe_ips);
        neighbors = collect_arp_neighbors();
    }

    build_dashboard_devices_with_environment(raw, share_devices, &local_probe_ips, &neighbors)
}

/// Maps already-fetched device and transfer-device projections without probing
/// the local interfaces, ARP table, or network. Used by endpoint-injected
/// refreshes and tests; production callers use [`build_dashboard_devices`].
pub fn build_dashboard_devices_pure(
    raw: Value,
    share_devices: Vec<ShareTransferDevice>,
) -> Vec<DashboardDevice> {
    build_dashboard_devices_with_environment(raw, share_devices, &[], &HashMap::new())
}

fn build_dashboard_devices_with_environment(
    raw: Value,
    share_devices: Vec<ShareTransferDevice>,
    local_probe_ips: &[Ipv4Addr],
    neighbors: &HashMap<String, String>,
) -> Vec<DashboardDevice> {
    let mut share_by_mac = HashMap::new();
    for item in &share_devices {
        if let Some(mac) = item.mac_address.as_deref().and_then(normalize_mac_address) {
            share_by_mac.entry(mac).or_insert_with(|| item.clone());
        }
    }
    let single_share_fallback = if share_devices.len() == 1 {
        share_devices.first().cloned()
    } else {
        None
    };

    let items = value_to_array(raw);
    let device_total = items.len();
    let mut devices = Vec::new();
    for item in items {
        let raw_mac = json_field_to_string(&item, "macAddress")
            .or_else(|| json_field_to_string(&item, "mac"));
        let normalized_mac = raw_mac.as_deref().and_then(normalize_mac_address);
        let matched_share_by_mac = normalized_mac
            .as_ref()
            .and_then(|mac| share_by_mac.get(mac).cloned());
        let matched_share = matched_share_by_mac.clone().or_else(|| {
            if device_total == 1 {
                single_share_fallback.clone()
            } else {
                None
            }
        });

        let transfer_host = matched_share.as_ref().and_then(|value| value.host.clone());
        let share_ip = transfer_host
            .as_deref()
            .and_then(extract_ipv4_from_transfer_host);
        let api_ip = extract_device_ip(&item).or(share_ip.clone());
        let arp_ip = normalized_mac
            .as_ref()
            .and_then(|mac| neighbors.get(mac).cloned());
        let same_lan_by_ip = api_ip
            .as_deref()
            .and_then(parse_ipv4_from_text)
            .map(|remote| {
                local_probe_ips
                    .iter()
                    .any(|local| in_same_lan_c24(*local, remote))
            })
            .unwrap_or(false);
        let login_status = json_field_to_string(&item, "loginStatus").or_else(|| {
            matched_share
                .as_ref()
                .and_then(|value| value.status.clone())
        });
        let same_lan_by_share = transfer_host.is_some();
        let same_lan_confident = same_lan_by_share || arp_ip.is_some() || same_lan_by_ip;
        let same_lan_fallback = !same_lan_confident
            && device_total == 1
            && !local_probe_ips.is_empty()
            && is_online_status(login_status.as_deref());
        let same_lan = same_lan_confident || same_lan_fallback;
        let same_lan_reason = if same_lan_by_share {
            if matched_share_by_mac.is_some() {
                Some("share_socket_mac".to_string())
            } else {
                Some("share_socket_single_fallback".to_string())
            }
        } else if arp_ip.is_some() {
            Some("mac_arp".to_string())
        } else if same_lan_by_ip {
            Some("same_subnet".to_string())
        } else if same_lan_fallback {
            Some("single_device_online_fallback".to_string())
        } else {
            None
        };

        let transfer_host = transfer_host.or_else(|| {
            if same_lan {
                api_ip.as_ref().map(|ip| format!("http://{ip}"))
            } else {
                None
            }
        });

        devices.push(DashboardDevice {
            id: json_field_to_string(&item, "id")
                .or_else(|| json_field_to_string(&item, "_id"))
                .or_else(|| json_field_to_string(&item, "deviceId")),
            model: json_field_to_string(&item, "model")
                .or_else(|| json_field_to_string(&item, "deviceModel"))
                .or_else(|| matched_share.as_ref().and_then(|value| value.model.clone())),
            mac_address: raw_mac.or_else(|| {
                matched_share
                    .as_ref()
                    .and_then(|value| value.mac_address.clone())
            }),
            ip_address: api_ip,
            login_status,
            latest_login_time: json_field_to_string(&item, "latestLoginTime"),
            latest_logout_time: json_field_to_string(&item, "latestLogoutTime"),
            locked: parse_bool_field(&item, "isLock").or_else(|| parse_bool_field(&item, "locked")),
            same_lan,
            lan_ip: arp_ip.or(share_ip),
            transfer_host,
            same_lan_reason,
        });
    }

    devices
}

pub fn parse_ipv4_from_text(text: &str) -> Option<Ipv4Addr> {
    for token in text.split(|ch: char| !(ch.is_ascii_digit() || ch == '.')) {
        if token.is_empty() {
            continue;
        }
        if let Ok(ip) = token.parse::<Ipv4Addr>() {
            return Some(ip);
        }
    }
    None
}

pub fn normalize_mac_address(text: &str) -> Option<String> {
    let hex: String = text.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    let mut out = String::with_capacity(17);
    for (idx, ch) in lower.chars().enumerate() {
        if idx > 0 && idx % 2 == 0 {
            out.push(':');
        }
        out.push(ch);
    }
    Some(out)
}

pub fn normalize_transfer_host_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://")
        && !trimmed.starts_with("http://")
        && !trimmed.starts_with("https://")
    {
        return None;
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(parsed.to_string())
}

pub fn extract_ipv4_from_transfer_host(host_url: &str) -> Option<String> {
    let parsed = url::Url::parse(host_url).ok()?;
    let host = parsed.host_str()?;
    host.parse::<Ipv4Addr>().ok().map(|ip| ip.to_string())
}

pub fn is_local_transfer_host(host: &str) -> bool {
    let lowered = host.trim().to_ascii_lowercase();
    if lowered == "localhost" || lowered.ends_with(".local") {
        return true;
    }
    match lowered.parse::<Ipv4Addr>() {
        Ok(ip) => is_private_or_cgnat_ipv4(ip) || ip.is_loopback() || ip.is_link_local(),
        Err(_) => false,
    }
}

pub fn extract_device_ip(item: &Value) -> Option<String> {
    [
        "ipAddress",
        "ip",
        "localIp",
        "localIP",
        "lanIp",
        "deviceIp",
        "lastIp",
        "latestIp",
    ]
    .iter()
    .find_map(|key| json_field_to_string(item, key))
}

#[cfg(target_os = "windows")]
fn command_output(program: &str, args: &[&str]) -> Option<Output> {
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

#[cfg(not(target_os = "windows"))]
fn command_output(program: &str, args: &[&str]) -> Option<Output> {
    Command::new(program).args(args).output().ok()
}

#[cfg(target_os = "windows")]
fn collect_arp_neighbors() -> HashMap<String, String> {
    let output = match command_output("arp", &["-a"]) {
        Some(output) if output.status.success() => output,
        _ => return HashMap::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut neighbors = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Interface:") {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(ip_token) = parts.next() else {
            continue;
        };
        let Some(mac_token) = parts.next() else {
            continue;
        };
        let Some(ip) = parse_ipv4_from_text(ip_token).map(|value| value.to_string()) else {
            continue;
        };
        let Some(mac) = normalize_mac_address(mac_token) else {
            continue;
        };
        neighbors.insert(mac, ip);
    }
    neighbors
}

#[cfg(not(target_os = "windows"))]
fn collect_arp_neighbors() -> HashMap<String, String> {
    let output = match command_output("arp", &["-an"]) {
        Some(output) if output.status.success() => output,
        _ => return HashMap::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut neighbors = HashMap::new();
    for line in text.lines() {
        let Some(open_idx) = line.find('(') else {
            continue;
        };
        let after_open = &line[(open_idx + 1)..];
        let Some(close_rel) = after_open.find(')') else {
            continue;
        };
        let ip_raw = &after_open[..close_rel];
        let Some(ip) = parse_ipv4_from_text(ip_raw).map(|value| value.to_string()) else {
            continue;
        };

        let Some(at_idx) = line.find(" at ") else {
            continue;
        };
        let after_at = &line[(at_idx + 4)..];
        let mac_token = after_at.split_whitespace().next().unwrap_or_default();
        let Some(mac) = normalize_mac_address(mac_token) else {
            continue;
        };
        neighbors.insert(mac, ip);
    }
    neighbors
}

#[cfg(target_os = "windows")]
fn collect_local_ipv4_candidates() -> Vec<(String, Ipv4Addr)> {
    let output = match command_output("ipconfig", &[]) {
        Some(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    let mut result = Vec::new();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_iface = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !line.starts_with(' ') && trimmed.ends_with(':') {
            current_iface = trimmed.trim_end_matches(':').to_string();
            continue;
        }

        if !trimmed.contains("IPv4") {
            continue;
        }

        let iface = current_iface.trim();
        if iface.is_empty() {
            continue;
        }

        let Some(ip) = parse_ipv4_from_text(trimmed) else {
            continue;
        };
        if ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        if !is_private_or_cgnat_ipv4(ip) {
            continue;
        }
        result.push((iface.to_string(), ip));
    }

    result.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.octets().cmp(&b.1.octets())));
    result.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    result
}

#[cfg(not(target_os = "windows"))]
fn collect_local_ipv4_candidates() -> Vec<(String, Ipv4Addr)> {
    let output = match command_output("ifconfig", &[]) {
        Some(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    let mut result = Vec::new();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_iface = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !line.starts_with('\t') && line.contains(':') {
            current_iface = line
                .split(':')
                .next()
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            continue;
        }

        if !trimmed.starts_with("inet ") {
            continue;
        }
        let iface = current_iface.trim();
        if iface.is_empty()
            || iface.starts_with("lo")
            || iface.starts_with("utun")
            || iface.starts_with("awdl")
            || iface.starts_with("llw")
            || iface.starts_with("gif")
            || iface.starts_with("stf")
            || iface.starts_with("anpi")
        {
            continue;
        }

        let ip_token = trimmed.split_whitespace().nth(1).unwrap_or_default();
        let Ok(ip) = ip_token.parse::<Ipv4Addr>() else {
            continue;
        };
        if ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        if !is_private_or_cgnat_ipv4(ip) {
            continue;
        }
        result.push((iface.to_string(), ip));
    }

    result.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.octets().cmp(&b.1.octets())));
    result.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    result
}

fn detect_local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr.ip() {
        IpAddr::V4(ipv4) => Some(ipv4),
        IpAddr::V6(_) => None,
    }
}

fn has_any_mac_match(neighbors: &HashMap<String, String>, target_macs: &[String]) -> bool {
    target_macs
        .iter()
        .filter_map(|value| normalize_mac_address(value))
        .any(|mac| neighbors.contains_key(&mac))
}

fn warmup_arp_by_udp_sweep(local_ips: &[Ipv4Addr]) {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return;
    };
    for local in local_ips {
        let local_octets = local.octets();
        for host in 1u8..=254u8 {
            if host == local_octets[3] {
                continue;
            }
            let target = SocketAddrV4::new(
                Ipv4Addr::new(local_octets[0], local_octets[1], local_octets[2], host),
                9,
            );
            let _ = socket.send_to(&[0x53], target);
        }
    }
    thread::sleep(Duration::from_millis(400));
}

fn is_private_or_cgnat_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    if octets[0] == 10 {
        return true;
    }
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn in_same_lan_c24(local: Ipv4Addr, other: Ipv4Addr) -> bool {
    let a = local.octets();
    let b = other.octets();
    a[0] == b[0] && a[1] == b[1] && a[2] == b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mac_formats_consistently() {
        assert_eq!(
            normalize_mac_address("AA-BB-CC-DD-EE-FF").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
    }

    #[test]
    fn local_transfer_hosts_are_restricted() {
        assert!(is_local_transfer_host("192.168.1.10"));
        assert!(is_local_transfer_host("localhost"));
        assert!(!is_local_transfer_host("8.8.8.8"));
    }

    #[test]
    fn transfer_host_is_normalized_to_http_when_missing_scheme() {
        assert_eq!(
            normalize_transfer_host_url("192.168.1.10:8080").as_deref(),
            Some("http://192.168.1.10:8080/")
        );
    }

    #[test]
    fn transfer_host_rejects_non_http_scheme() {
        assert_eq!(normalize_transfer_host_url("ftp://192.168.1.10"), None);
    }

    #[test]
    fn extracts_ipv4_from_transfer_host_url() {
        assert_eq!(
            extract_ipv4_from_transfer_host("http://192.168.1.10:8080/").as_deref(),
            Some("192.168.1.10")
        );
    }
}
