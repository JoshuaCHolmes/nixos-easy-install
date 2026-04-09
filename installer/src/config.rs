//! Installation configuration types

use serde::{Deserialize, Serialize};

/// The configuration that gets written to install-config.json
/// and read by the NixOS installer initrd
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    pub version: u32,
    pub install_type: String,  // "loopback" or "partition"
    pub hostname: String,
    pub username: String,
    pub password_hash: String,
    pub flake: FlakeConfig,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loopback: Option<LoopbackConfig>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<PartitionConfig>,
    
    pub options: InstallOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeConfig {
    #[serde(rename = "type")]
    pub config_type: String,  // "starter", "minimal", "url", "local"
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopbackConfig {
    pub target_dir: String,
    pub size_gb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionConfig {
    pub root: String,
    pub boot: String,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOptions {
    pub encrypt: bool,
    pub secure_boot: bool,
}

impl InstallConfig {
    pub fn new_loopback(
        hostname: String,
        username: String,
        password_hash: String,
        flake_type: &str,
        flake_url: Option<String>,
        size_gb: u32,
    ) -> Self {
        Self {
            version: 1,
            install_type: "loopback".to_string(),
            hostname: hostname.clone(),
            username,
            password_hash,
            flake: FlakeConfig {
                config_type: flake_type.to_string(),
                url: flake_url,
                hostname: Some(hostname),
            },
            loopback: Some(LoopbackConfig {
                target_dir: "C:\\NixOS".to_string(),
                size_gb,
            }),
            partition: None,
            options: InstallOptions {
                encrypt: false,
                secure_boot: true,
            },
        }
    }
    
    pub fn new_partition(
        hostname: String,
        username: String,
        password_hash: String,
        flake_type: &str,
        flake_url: Option<String>,
        root_partition: String,
        boot_partition: String,
        swap_partition: Option<String>,
        encrypt: bool,
    ) -> Self {
        Self {
            version: 1,
            install_type: "partition".to_string(),
            hostname: hostname.clone(),
            username,
            password_hash,
            flake: FlakeConfig {
                config_type: flake_type.to_string(),
                url: flake_url,
                hostname: Some(hostname),
            },
            loopback: None,
            partition: Some(PartitionConfig {
                root: root_partition,
                boot: boot_partition,
                swap: swap_partition,
            }),
            options: InstallOptions {
                encrypt,
                secure_boot: true,
            },
        }
    }
    
    /// Serialize to JSON for writing to install-config.json
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Hash a password using SHA-512 crypt format (compatible with NixOS hashedPassword)
pub fn hash_password(password: &str) -> String {
    // TODO: Implement proper password hashing
    // For now, return a placeholder that will need to be replaced
    // with actual mkpasswd -m sha-512 output
    format!("$6$rounds=10000$placeholder${}", password)
}
