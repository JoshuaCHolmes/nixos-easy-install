//! System utilities - Windows API interactions for detection and validation
//! 
//! SAFETY: This module focuses on READ-ONLY operations for system detection.
//! Any modifying operations are clearly marked and isolated.

// Some fields are reserved for future use (e.g., full partition install)
#![allow(dead_code)]

use anyhow::{Context, Result};
use tracing::{info, debug};

// ============================================================================
// System Information (Read-Only)
// ============================================================================

/// Comprehensive system information gathered before installation
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// Total physical RAM in bytes
    pub total_memory: u64,
    
    /// Whether the system booted via UEFI (vs legacy BIOS)
    pub is_uefi: bool,
    
    /// Whether Secure Boot is currently enabled
    pub secure_boot_enabled: bool,
    
    /// Windows version string
    pub windows_version: String,
    
    /// EFI System Partition info (if UEFI)
    pub esp: Option<EspInfo>,
    
    /// Available disks
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug, Clone)]
pub struct EspInfo {
    /// Drive letter (e.g., "S:" or may be unmounted)
    pub drive_letter: Option<String>,
    
    /// Partition GUID
    pub partition_guid: String,
    
    /// Size in bytes
    pub size: u64,
    
    /// Free space in bytes
    pub free_space: u64,
    
    /// Physical disk number
    pub disk_number: u32,
    
    /// Partition number on that disk
    pub partition_number: u32,
    
    /// Mount point path (e.g., S:\)
    pub mount_point: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    /// Disk number (0, 1, 2...)
    pub number: u32,
    
    /// Friendly name
    pub name: String,
    
    /// Total size in bytes
    pub size: u64,
    
    /// Whether this is the system disk (contains Windows)
    pub is_system_disk: bool,
    
    /// Partition style: "GPT" or "MBR"
    pub partition_style: String,
    
    /// Partitions on this disk
    pub partitions: Vec<PartitionInfo>,
}

#[derive(Debug, Clone)]
pub struct PartitionInfo {
    /// Partition number
    pub number: u32,
    
    /// Drive letter if mounted
    pub drive_letter: Option<String>,
    
    /// Size in bytes
    pub size: u64,
    
    /// Free space in bytes (if accessible)
    pub free_space: Option<u64>,
    
    /// Filesystem type
    pub filesystem: String,
    
    /// Partition type (esp, primary, etc.)
    pub partition_type: String,
    
    /// Volume label
    pub label: String,
}

// ============================================================================
// Detection Functions (Read-Only)
// ============================================================================

/// Gather all system information
/// 
/// SAFETY: This function only reads system state, makes no modifications.
pub fn detect_system() -> Result<SystemInfo> {
    info!("Detecting system configuration...");
    
    let is_uefi = detect_uefi_mode()?;
    debug!("UEFI mode: {}", is_uefi);
    
    let secure_boot = if is_uefi {
        detect_secure_boot()?
    } else {
        false
    };
    debug!("Secure Boot: {}", secure_boot);
    
    let esp = if is_uefi {
        detect_esp().ok()
    } else {
        None
    };
    debug!("ESP: {:?}", esp);
    
    let disks = detect_disks()?;
    debug!("Found {} disks", disks.len());
    
    let memory = detect_memory()?;
    let windows_version = detect_windows_version()?;
    
    Ok(SystemInfo {
        total_memory: memory,
        is_uefi,
        secure_boot_enabled: secure_boot,
        windows_version,
        esp,
        disks,
    })
}

/// Check if system booted in UEFI mode
#[cfg(windows)]
fn detect_uefi_mode() -> Result<bool> {
    use std::process::Command;
    
    // GetFirmwareEnvironmentVariable returns ERROR_INVALID_FUNCTION on BIOS
    // We can also check via bcdedit or registry
    let _output = Command::new("powershell")
        .args(["-Command", "[Environment]::Is64BitOperatingSystem -and (Test-Path 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\SecureBoot')"])
        .output()
        .context("Failed to detect UEFI mode")?;
    
    // Check if EFI system partition exists
    let efi_check = Command::new("powershell")
        .args(["-Command", "(Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' }) -ne $null"])
        .output()
        .context("Failed to check for EFI partition")?;
    
    let result = String::from_utf8_lossy(&efi_check.stdout)
        .trim()
        .to_lowercase();
    
    Ok(result == "true")
}

#[cfg(not(windows))]
fn detect_uefi_mode() -> Result<bool> {
    // On Linux, check for /sys/firmware/efi
    Ok(std::path::Path::new("/sys/firmware/efi").exists())
}

/// Check if Secure Boot is enabled
#[cfg(windows)]
fn detect_secure_boot() -> Result<bool> {
    use std::process::Command;
    
    let output = Command::new("powershell")
        .args(["-Command", "Confirm-SecureBootUEFI"])
        .output()
        .context("Failed to check Secure Boot status")?;
    
    let result = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    
    Ok(result == "true")
}

#[cfg(not(windows))]
fn detect_secure_boot() -> Result<bool> {
    // On Linux, check mokutil or /sys/firmware/efi/efivars
    Ok(false)
}

/// Find the EFI System Partition
#[cfg(windows)]
fn detect_esp() -> Result<EspInfo> {
    use std::process::Command;
    
    // Use PowerShell to find ESP
    let script = r#"
        $esp = Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } | Select-Object -First 1
        if ($esp) {
            $volume = Get-Volume -Partition $esp -ErrorAction SilentlyContinue
            [PSCustomObject]@{
                DiskNumber = $esp.DiskNumber
                PartitionNumber = $esp.PartitionNumber
                Size = $esp.Size
                DriveLetter = $volume.DriveLetter
                FreeSpace = $volume.SizeRemaining
                Guid = $esp.Guid
            } | ConvertTo-Json
        }
    "#;
    
    let output = Command::new("powershell")
        .args(["-Command", script])
        .output()
        .context("Failed to detect ESP")?;
    
    let json = String::from_utf8_lossy(&output.stdout);
    
    if json.trim().is_empty() {
        anyhow::bail!("No EFI System Partition found");
    }
    
    // Parse the JSON response
    let parsed: serde_json::Value = serde_json::from_str(json.trim())
        .context("Failed to parse ESP info")?;
    
    let drive_letter = parsed["DriveLetter"].as_str().map(|s| format!("{}:", s));
    let mount_point = drive_letter.as_ref()
        .map(|d| std::path::PathBuf::from(format!("{}\\", d)))
        .unwrap_or_else(|| std::path::PathBuf::from(""));
    
    Ok(EspInfo {
        disk_number: parsed["DiskNumber"].as_u64().unwrap_or(0) as u32,
        partition_number: parsed["PartitionNumber"].as_u64().unwrap_or(0) as u32,
        size: parsed["Size"].as_u64().unwrap_or(0),
        drive_letter,
        free_space: parsed["FreeSpace"].as_u64().unwrap_or(0),
        partition_guid: parsed["Guid"].as_str().unwrap_or("").to_string(),
        mount_point,
    })
}

#[cfg(not(windows))]
fn detect_esp() -> Result<EspInfo> {
    anyhow::bail!("ESP detection not implemented for this platform")
}

/// Enumerate all disks and partitions
#[cfg(windows)]
fn detect_disks() -> Result<Vec<DiskInfo>> {
    use std::process::Command;
    
    let script = r#"
        Get-Disk | ForEach-Object {
            $disk = $_
            $partitions = Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue | ForEach-Object {
                $part = $_
                $vol = Get-Volume -Partition $part -ErrorAction SilentlyContinue
                [PSCustomObject]@{
                    Number = $part.PartitionNumber
                    DriveLetter = $part.DriveLetter
                    Size = $part.Size
                    FreeSpace = $vol.SizeRemaining
                    FileSystem = $vol.FileSystem
                    Type = $part.Type
                    Label = $vol.FileSystemLabel
                }
            }
            [PSCustomObject]@{
                Number = $disk.Number
                Name = $disk.FriendlyName
                Size = $disk.Size
                IsSystem = $disk.IsSystem
                PartitionStyle = $disk.PartitionStyle
                Partitions = $partitions
            }
        } | ConvertTo-Json -Depth 3
    "#;
    
    let output = Command::new("powershell")
        .args(["-Command", script])
        .output()
        .context("Failed to enumerate disks")?;
    
    let json = String::from_utf8_lossy(&output.stdout);
    
    if json.trim().is_empty() {
        return Ok(vec![]);
    }
    
    // Parse JSON - could be array or single object
    let parsed: serde_json::Value = serde_json::from_str(json.trim())
        .context("Failed to parse disk info")?;
    
    let disks_array = if parsed.is_array() {
        parsed.as_array().unwrap().clone()
    } else {
        vec![parsed]
    };
    
    let mut disks = Vec::new();
    
    for disk_json in disks_array {
        let partitions_json = disk_json["Partitions"].as_array();
        
        let partitions: Vec<PartitionInfo> = partitions_json
            .map(|arr| {
                arr.iter().map(|p| PartitionInfo {
                    number: p["Number"].as_u64().unwrap_or(0) as u32,
                    drive_letter: p["DriveLetter"].as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| format!("{}:", s)),
                    size: p["Size"].as_u64().unwrap_or(0),
                    free_space: p["FreeSpace"].as_u64(),
                    filesystem: p["FileSystem"].as_str().unwrap_or("").to_string(),
                    partition_type: p["Type"].as_str().unwrap_or("").to_string(),
                    label: p["Label"].as_str().unwrap_or("").to_string(),
                }).collect()
            })
            .unwrap_or_default();
        
        disks.push(DiskInfo {
            number: disk_json["Number"].as_u64().unwrap_or(0) as u32,
            name: disk_json["Name"].as_str().unwrap_or("Unknown").to_string(),
            size: disk_json["Size"].as_u64().unwrap_or(0),
            is_system_disk: disk_json["IsSystem"].as_bool().unwrap_or(false),
            partition_style: disk_json["PartitionStyle"].as_str().unwrap_or("Unknown").to_string(),
            partitions,
        });
    }
    
    Ok(disks)
}

#[cfg(not(windows))]
fn detect_disks() -> Result<Vec<DiskInfo>> {
    Ok(vec![])
}

/// Get total system memory
#[cfg(windows)]
fn detect_memory() -> Result<u64> {
    use std::process::Command;
    
    let output = Command::new("powershell")
        .args(["-Command", "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"])
        .output()
        .context("Failed to detect memory")?;
    
    let mem_str = String::from_utf8_lossy(&output.stdout);
    mem_str.trim().parse().context("Failed to parse memory value")
}

#[cfg(not(windows))]
fn detect_memory() -> Result<u64> {
    Ok(16 * 1024 * 1024 * 1024) // 16GB placeholder
}

/// Get Windows version string
#[cfg(windows)]
fn detect_windows_version() -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("powershell")
        .args(["-Command", "(Get-CimInstance Win32_OperatingSystem).Caption"])
        .output()
        .context("Failed to detect Windows version")?;
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(windows))]
fn detect_windows_version() -> Result<String> {
    Ok("Not Windows".to_string())
}

// ============================================================================
// Validation Functions (Read-Only)
// ============================================================================

/// Validation result with detailed issues
#[derive(Debug)]
pub struct ValidationResult {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Validate system meets minimum requirements for installation
pub fn validate_requirements(info: &SystemInfo) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    
    // Check memory (minimum 2GB, recommended 4GB)
    let mem_gb = info.total_memory / (1024 * 1024 * 1024);
    if mem_gb < 2 {
        errors.push(format!("Insufficient memory: {}GB (minimum 2GB required)", mem_gb));
    } else if mem_gb < 4 {
        warnings.push(format!("Low memory: {}GB (4GB+ recommended)", mem_gb));
    }
    
    // Check UEFI
    if !info.is_uefi {
        warnings.push("Legacy BIOS detected. UEFI is recommended.".to_string());
    }
    
    // Check ESP exists and has space
    if info.is_uefi {
        match &info.esp {
            Some(esp) => {
                let esp_free_mb = esp.free_space / (1024 * 1024);
                if esp_free_mb < 100 {
                    errors.push(format!(
                        "Insufficient ESP space: {}MB free (100MB required)",
                        esp_free_mb
                    ));
                }
            }
            None => {
                errors.push("No EFI System Partition found".to_string());
            }
        }
    }
    
    // Check for available disk space
    let max_free_space: u64 = info.disks.iter()
        .flat_map(|d| &d.partitions)
        .filter_map(|p| p.free_space)
        .max()
        .unwrap_or(0);
    
    let free_gb = max_free_space / (1024 * 1024 * 1024);
    if free_gb < 20 {
        errors.push(format!(
            "Insufficient disk space: {}GB free (20GB minimum required)",
            free_gb
        ));
    } else if free_gb < 50 {
        warnings.push(format!(
            "Limited disk space: {}GB free (50GB+ recommended)",
            free_gb
        ));
    }
    
    ValidationResult {
        passed: errors.is_empty(),
        errors,
        warnings,
    }
}

// ============================================================================
// Privilege Management
// ============================================================================

/// Check if the current process is running with administrator privileges
#[cfg(windows)]
pub fn is_admin() -> bool {
    use std::process::Command;
    
    // Use PowerShell to check elevation
    let output = Command::new("powershell")
        .args(["-Command", "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"])
        .output();
    
    match output {
        Ok(out) => {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .to_lowercase() == "true"
        }
        Err(_) => false,
    }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Re-launch the application with administrator privileges
#[cfg(windows)]
pub fn elevate() -> Result<()> {
    use std::process::Command;
    
    let exe = std::env::current_exe()?;
    
    info!("Requesting administrator privileges...");
    
    // Use PowerShell Start-Process with -Verb RunAs for UAC prompt
    let status = Command::new("powershell")
        .args([
            "-Command",
            &format!(
                "Start-Process '{}' -Verb RunAs",
                exe.display()
            ),
        ])
        .status()
        .context("Failed to request elevation")?;
    
    if !status.success() {
        anyhow::bail!("User declined administrator privileges");
    }
    
    std::process::exit(0);
}

#[cfg(not(windows))]
pub fn elevate() -> Result<()> {
    anyhow::bail!("Elevation not supported on this platform - run with sudo");
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format bytes as human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    
    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
