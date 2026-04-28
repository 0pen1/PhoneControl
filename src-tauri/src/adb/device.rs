use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub serial: String,
    pub status: String, // online / offline / unauthorized
    pub model: String,
    pub battery: i32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub server_host: String,
    pub server_port: u16,
}

/// Parse `adb devices` output into (serial, status) pairs.
pub fn parse_adb_devices(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .skip(1) // skip "List of devices attached"
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Build adb args prefix for a remote server.
pub fn server_args(host: &str, port: u16) -> Vec<String> {
    if (host == "localhost" || host == "127.0.0.1") && port == 5037 {
        vec![]
    } else {
        vec!["-H".into(), host.into(), "-P".into(), port.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_devices_normal() {
        let output =
            "List of devices attached\nemulator-5554\tdevice\n192.168.1.100:5555\tdevice\n";
        let devices = parse_adb_devices(output);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].0, "emulator-5554");
        assert_eq!(devices[0].1, "device");
        assert_eq!(devices[1].0, "192.168.1.100:5555");
    }

    #[test]
    fn test_parse_devices_unauthorized() {
        let output = "List of devices attached\nABC123\tunauthorized\n";
        let devices = parse_adb_devices(output);
        assert_eq!(devices[0].1, "unauthorized");
    }

    #[test]
    fn test_parse_devices_empty() {
        let output = "List of devices attached\n";
        let devices = parse_adb_devices(output);
        assert!(devices.is_empty());
    }

    #[test]
    fn test_server_args_local() {
        assert!(server_args("127.0.0.1", 5037).is_empty());
        assert!(server_args("localhost", 5037).is_empty());
    }

    #[test]
    fn test_server_args_remote() {
        let args = server_args("192.168.1.1", 5037);
        assert_eq!(args, vec!["-H", "192.168.1.1", "-P", "5037"]);
    }
}
