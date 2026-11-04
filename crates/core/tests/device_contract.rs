//! Device projection contracts at the public core boundary (S2).
//!
//! These tests supply explicit loopback-safe network evidence so they never
//! probe the host interfaces, ARP table, WebSocket service, or a real device.

use boox_core::device::build_dashboard_devices_with_environment;
use std::collections::HashMap;

#[test]
fn arp_only_address_remains_a_transfer_candidate() {
    let raw = serde_json::json!([{
        "id": "device-1",
        "macAddress": "AA:BB:CC:DD:EE:FF",
        "model": "BOOX Note",
        "loginStatus": "online"
    }]);
    let neighbors = HashMap::from([("aa:bb:cc:dd:ee:ff".to_string(), "192.168.1.50".to_string())]);

    let devices = build_dashboard_devices_with_environment(raw, Vec::new(), &[], &neighbors);

    assert_eq!(devices.len(), 1);
    let device = &devices[0];
    assert!(device.same_lan);
    assert_eq!(device.same_lan_reason.as_deref(), Some("mac_arp"));
    assert_eq!(device.lan_ip.as_deref(), Some("192.168.1.50"));
    assert_eq!(device.transfer_host.as_deref(), Some("http://192.168.1.50"));
}

#[test]
fn arp_candidate_wins_over_a_nonlocal_cloud_address() {
    use std::net::Ipv4Addr;

    let local_ip = Ipv4Addr::new(192, 168, 1, 50);
    let raw = serde_json::json!([{
        "id": "device-1",
        "macAddress": "AA:BB:CC:DD:EE:FF",
        "model": "BOOX Note",
        "loginStatus": "online",
        "ipAddress": "198.51.100.9"
    }]);
    let neighbors = HashMap::from([("aa:bb:cc:dd:ee:ff".to_string(), local_ip.to_string())]);

    let devices =
        build_dashboard_devices_with_environment(raw, Vec::new(), &[local_ip], &neighbors);

    assert_eq!(devices.len(), 1);
    let device = &devices[0];
    assert!(device.same_lan);
    assert_eq!(device.same_lan_reason.as_deref(), Some("mac_arp"));
    assert_eq!(device.ip_address.as_deref(), Some("198.51.100.9"));
    assert_eq!(device.lan_ip, Some(local_ip.to_string()));
    assert_eq!(device.transfer_host, Some(format!("http://{local_ip}")));
}
