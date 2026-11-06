// discovery.rs — Announce this PA on the LAN (FEAT-MP1)
//
// `_pocket-agent._tcp`, port 8650, TXT id=<pa_id> name=<computer name> v=1.
// The phone browses with Android's NsdManager; typing the IP stays the
// fallback, so any failure here is logged and nothing else.

use std::sync::OnceLock;

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_pocket-agent._tcp.local.";
pub const PROTOCOL_VERSION: &str = "1";

/// Kept for the life of the process: dropping the daemon withdraws the service.
static DAEMON: OnceLock<ServiceDaemon> = OnceLock::new();

/// Instance and host labels: unique per PA (two computers may share a name).
fn labels(pa_id: &str) -> (String, String) {
    let short: String = pa_id.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect();
    (format!("pocket-agent-{short}"), format!("pa-{short}.local."))
}

fn service(pa_id: &str, pa_name: &str, port: u16) -> Result<ServiceInfo, mdns_sd::Error> {
    let (instance, host) = labels(pa_id);
    let txt = [("id", pa_id), ("name", pa_name), ("v", PROTOCOL_VERSION)];
    Ok(ServiceInfo::new(SERVICE_TYPE, &instance, &host, "", port, &txt[..])?.enable_addr_auto())
}

pub fn advertise(port: u16) {
    let (pa_id, pa_name) = super::pairing::identity();
    let result = ServiceDaemon::new().and_then(|daemon| {
        daemon.register(service(pa_id, pa_name, port)?)?;
        Ok(daemon)
    });
    match result {
        Ok(daemon) => {
            let _ = DAEMON.set(daemon);
            eprintln!("[mdns] advertising {} on port {}", SERVICE_TYPE, port);
        }
        Err(e) => eprintln!("[mdns] not advertising (phones can still use the IP address): {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_carries_id_name_and_version() {
        let info = service("0f3a9c21-1111-4222-8333-444455556666", "Kevin 的 MacBook", 8650).unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 8650);
        assert_eq!(info.get_fullname(), "pocket-agent-0f3a9c21._pocket-agent._tcp.local.");
        assert_eq!(info.get_property_val_str("id"), Some("0f3a9c21-1111-4222-8333-444455556666"));
        assert_eq!(info.get_property_val_str("name"), Some("Kevin 的 MacBook"));
        assert_eq!(info.get_property_val_str("v"), Some("1"));
    }
}
