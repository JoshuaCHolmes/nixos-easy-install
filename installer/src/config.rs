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
/// 
/// This produces the same output as `mkpasswd -m sha-512`
pub fn hash_password(password: &str) -> String {
    // Generate random salt (16 characters from ./0-9A-Za-z)
    let salt = generate_salt(16);
    
    // SHA-512 crypt with 5000 rounds (default)
    sha512_crypt(password, &salt, 5000)
}

/// Generate a random salt for password hashing
fn generate_salt(len: usize) -> String {
    use rand::Rng;
    
    const CHARSET: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// SHA-512 crypt implementation
/// Compatible with glibc's crypt() and mkpasswd
fn sha512_crypt(password: &str, salt: &str, rounds: u32) -> String {
    use sha2::{Sha512, Digest};
    
    let password = password.as_bytes();
    let salt = salt.as_bytes();
    
    // Step 1-8: Initial digest B
    let mut digest_b = Sha512::new();
    digest_b.update(password);
    digest_b.update(salt);
    digest_b.update(password);
    let hash_b = digest_b.finalize();
    
    // Step 9-12: Initial digest A
    let mut digest_a = Sha512::new();
    digest_a.update(password);
    digest_a.update(salt);
    
    // Add bytes from B based on password length
    let mut remaining = password.len();
    while remaining > 64 {
        digest_a.update(&hash_b[..]);
        remaining -= 64;
    }
    digest_a.update(&hash_b[..remaining]);
    
    // Step 13-15: Process password length bits
    let mut len = password.len();
    while len > 0 {
        if len & 1 != 0 {
            digest_a.update(&hash_b[..]);
        } else {
            digest_a.update(password);
        }
        len >>= 1;
    }
    let mut hash_a = digest_a.finalize();
    
    // Step 16-19: Create DP
    let mut digest_dp = Sha512::new();
    for _ in 0..password.len() {
        digest_dp.update(password);
    }
    let hash_dp = digest_dp.finalize();
    
    // Step 20: Create P
    let mut p = Vec::with_capacity(password.len());
    let mut remaining = password.len();
    while remaining > 64 {
        p.extend_from_slice(&hash_dp[..]);
        remaining -= 64;
    }
    p.extend_from_slice(&hash_dp[..remaining]);
    
    // Step 21-22: Create DS
    let mut digest_ds = Sha512::new();
    for _ in 0..(16 + hash_a[0] as usize) {
        digest_ds.update(salt);
    }
    let hash_ds = digest_ds.finalize();
    
    // Step 23: Create S
    let mut s = Vec::with_capacity(salt.len());
    let mut remaining = salt.len();
    while remaining > 64 {
        s.extend_from_slice(&hash_ds[..]);
        remaining -= 64;
    }
    s.extend_from_slice(&hash_ds[..remaining]);
    
    // Step 24: Main rounds
    for i in 0..rounds {
        let mut digest_c = Sha512::new();
        
        if i & 1 != 0 {
            digest_c.update(&p);
        } else {
            digest_c.update(&hash_a[..]);
        }
        
        if i % 3 != 0 {
            digest_c.update(&s);
        }
        
        if i % 7 != 0 {
            digest_c.update(&p);
        }
        
        if i & 1 != 0 {
            digest_c.update(&hash_a[..]);
        } else {
            digest_c.update(&p);
        }
        
        hash_a = digest_c.finalize();
    }
    
    // Step 25: Encode result
    let hash_bytes: [u8; 64] = hash_a.into();
    let encoded = sha512_b64_encode(&hash_bytes);
    
    // Format output
    if rounds == 5000 {
        format!("$6${}${}", String::from_utf8_lossy(salt), encoded)
    } else {
        format!("$6$rounds={}${}${}", rounds, String::from_utf8_lossy(salt), encoded)
    }
}

/// Custom base64 encoding for SHA-512 crypt (different alphabet and order)
fn sha512_b64_encode(hash: &[u8; 64]) -> String {
    const B64: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    
    // SHA-512 crypt uses a specific byte order for encoding
    let order: [(usize, usize, usize); 21] = [
        (0, 21, 42), (22, 43, 1), (44, 2, 23), (3, 24, 45),
        (25, 46, 4), (47, 5, 26), (6, 27, 48), (28, 49, 7),
        (50, 8, 29), (9, 30, 51), (31, 52, 10), (53, 11, 32),
        (12, 33, 54), (34, 55, 13), (56, 14, 35), (15, 36, 57),
        (37, 58, 16), (59, 17, 38), (18, 39, 60), (40, 61, 19),
        (62, 20, 41),
    ];
    
    let mut result = String::with_capacity(86);
    
    for (a, b, c) in order.iter() {
        let v = ((hash[*a] as u32) << 16) | ((hash[*b] as u32) << 8) | (hash[*c] as u32);
        result.push(B64[(v & 0x3f) as usize] as char);
        result.push(B64[((v >> 6) & 0x3f) as usize] as char);
        result.push(B64[((v >> 12) & 0x3f) as usize] as char);
        result.push(B64[((v >> 18) & 0x3f) as usize] as char);
    }
    
    // Last byte (63) only contributes 2 characters
    let v = hash[63] as u32;
    result.push(B64[(v & 0x3f) as usize] as char);
    result.push(B64[((v >> 6) & 0x3f) as usize] as char);
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hash_password_format() {
        let hash = hash_password("test123");
        assert!(hash.starts_with("$6$"));
        assert!(hash.len() > 90); // $6$ + salt + $ + hash
    }
    
    #[test]
    fn test_hash_password_different_salts() {
        let hash1 = hash_password("test");
        let hash2 = hash_password("test");
        // Same password should produce different hashes (different salts)
        assert_ne!(hash1, hash2);
    }
}
