use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConfigFile {
    servers: Vec<ServerConfig>,
}

fn config_path() -> PathBuf {
    let mut p = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push(".phone_control");
    fs::create_dir_all(&p).ok();
    p.push("servers.json");
    p
}

pub fn load_servers() -> Vec<ServerConfig> {
    let path = config_path();
    if !path.exists() {
        return vec![ServerConfig {
            host: "127.0.0.1".into(),
            port: 5037,
            enabled: true,
        }];
    }
    let text = fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str::<ConfigFile>(&text)
        .map(|c| c.servers)
        .unwrap_or_else(|_| {
            vec![ServerConfig {
                host: "127.0.0.1".into(),
                port: 5037,
                enabled: true,
            }]
        })
}

pub fn save_servers(servers: &[ServerConfig]) -> Result<(), String> {
    let path = config_path();
    let data = ConfigFile {
        servers: servers.to_vec(),
    };
    let text = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_roundtrip() {
        let tmp = env::temp_dir().join("phone_control_test");
        fs::create_dir_all(&tmp).unwrap();
        let servers = vec![
            ServerConfig {
                host: "192.168.1.1".into(),
                port: 5037,
                enabled: true,
            },
            ServerConfig {
                host: "10.0.0.1".into(),
                port: 5555,
                enabled: false,
            },
        ];
        let data = ConfigFile {
            servers: servers.clone(),
        };
        let text = serde_json::to_string_pretty(&data).unwrap();
        let path = tmp.join("servers.json");
        fs::write(&path, &text).unwrap();
        let loaded: ConfigFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.servers.len(), 2);
        assert_eq!(loaded.servers[0].host, "192.168.1.1");
        assert_eq!(loaded.servers[1].port, 5555);
        assert!(!loaded.servers[1].enabled);
    }
}
