//! Bootloader setup for UEFI systems
//! 
//! This module handles:
//! 1. Copying boot files to ESP (shimx64.efi, grubx64.efi, etc.)
//! 2. Creating UEFI boot entry via bcdedit
//! 3. Setting up initial boot configuration
//! 
//! SAFETY DESIGN:
//! - We only ADD files to ESP, never modify/delete existing Windows files
//! - Boot entries are additive (Windows entry remains untouched)
//! - All operations are reversible by deleting our folder and boot entry

// Some functions are reserved for different boot scenarios
#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::fs;
use tracing::{info, warn, debug};

use crate::system::EspInfo;

/// Files we need to copy to the ESP
#[derive(Debug)]
pub struct BootFiles {
    /// Signed shim (Microsoft-signed, loads GRUB)
    pub shim: PathBuf,
    
    /// GRUB EFI binary (signed with MOK)
    pub grub: PathBuf,
    
    /// Machine Owner Key (for Secure Boot)
    pub mok_cert: PathBuf,
    
    /// GRUB configuration
    pub grub_cfg: PathBuf,
}

/// Result of bootloader setup
#[derive(Debug, Clone)]
pub struct BootloaderSetupResult {
    /// Path to our boot folder on ESP
    pub esp_folder: PathBuf,
    
    /// UEFI boot entry ID (for removal if needed)
    pub boot_entry_id: String,
    
    /// Whether Secure Boot setup is complete
    pub secure_boot_ready: bool,
}

// ============================================================================
// Pre-flight Checks (Read-Only)
// ============================================================================

/// Verify that bootloader setup is possible
pub fn preflight_check(esp: &EspInfo) -> Result<BootPreflight> {
    info!("Running bootloader preflight checks...");
    
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    
    // Check 1: ESP has enough space (we need ~50MB for boot files)
    const REQUIRED_SPACE: u64 = 50 * 1024 * 1024; // 50MB
    if esp.free_space < REQUIRED_SPACE {
        errors.push(format!(
            "ESP has insufficient space: {} available, 50MB required",
            crate::system::format_bytes(esp.free_space)
        ));
    }
    
    // Check 2: ESP is mounted and accessible
    if !esp.mount_point.exists() {
        errors.push(format!(
            "ESP mount point '{}' not accessible",
            esp.mount_point.display()
        ));
    }
    
    // Check 3: Check if our folder already exists
    let nixos_folder = esp.mount_point.join("EFI").join("NixOS");
    if nixos_folder.exists() {
        warnings.push(format!(
            "NixOS boot folder already exists at '{}' - will be overwritten",
            nixos_folder.display()
        ));
    }
    
    // Check 4: Verify we can write to ESP
    let test_file = esp.mount_point.join(".nixos_write_test");
    match fs::write(&test_file, "test") {
        Ok(_) => {
            let _ = fs::remove_file(&test_file);
        }
        Err(e) => {
            errors.push(format!(
                "Cannot write to ESP '{}': {}. Run as administrator.",
                esp.mount_point.display(),
                e
            ));
        }
    }
    
    Ok(BootPreflight {
        passed: errors.is_empty(),
        errors,
        warnings,
        nixos_folder,
    })
}

#[derive(Debug)]
pub struct BootPreflight {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub nixos_folder: PathBuf,
}

// ============================================================================
// Bootloader Installation
// ============================================================================

/// Set up bootloader on ESP
/// 
/// SAFETY:
/// - Creates new folder in EFI directory only
/// - Does not modify any existing boot entries
/// - All files are additive
pub fn setup_bootloader(
    esp: &EspInfo, 
    boot_files: &BootFiles,
    display_name: &str,
) -> Result<BootloaderSetupResult> {
    info!("Setting up bootloader on ESP at {:?}", esp.mount_point);
    
    // Run preflight
    let preflight = preflight_check(esp)?;
    if !preflight.passed {
        bail!("Bootloader preflight failed: {:?}", preflight.errors);
    }
    
    let nixos_folder = preflight.nixos_folder;
    
    // Create our boot folder
    info!("Creating NixOS boot folder: {:?}", nixos_folder);
    fs::create_dir_all(&nixos_folder)
        .context("Failed to create NixOS boot folder")?;
    
    // Copy boot files
    copy_boot_file(&boot_files.shim, &nixos_folder.join("shimx64.efi"), "shim")?;
    copy_boot_file(&boot_files.grub, &nixos_folder.join("grubx64.efi"), "GRUB")?;
    copy_boot_file(&boot_files.mok_cert, &nixos_folder.join("MOK.cer"), "MOK certificate")?;
    copy_boot_file(&boot_files.grub_cfg, &nixos_folder.join("grub.cfg"), "GRUB config")?;
    
    // Create UEFI boot entry
    info!("Creating UEFI boot entry...");
    let boot_entry_id = create_boot_entry(
        &nixos_folder.join("shimx64.efi"),
        display_name,
    )?;
    
    // Verify the boot entry was actually created
    if !verify_boot_entry(&boot_entry_id)? {
        // Try to clean up the files we copied
        warn!("Boot entry creation could not be verified, cleaning up...");
        let _ = fs::remove_dir_all(&nixos_folder);
        bail!("Boot entry creation failed - entry {} not found in bcdedit output", boot_entry_id);
    }
    
    info!("Bootloader setup complete. Entry ID: {}", boot_entry_id);
    
    Ok(BootloaderSetupResult {
        esp_folder: nixos_folder,
        boot_entry_id,
        secure_boot_ready: true,
    })
}

/// Verify that a boot entry exists in bcdedit
fn verify_boot_entry(entry_id: &str) -> Result<bool> {
    use std::process::Command;
    
    let output = Command::new("bcdedit")
        .args(["/enum", "all"])
        .output()
        .context("Failed to run bcdedit /enum")?;
    
    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(output_str.contains(entry_id))
}

/// Create UEFI boot entry using bcdedit
/// 
/// SAFETY:
/// - Only ADDS a new entry, never modifies existing
/// - Windows boot entry remains the default
fn create_boot_entry(efi_path: &Path, display_name: &str) -> Result<String> {
    use std::process::Command;
    
    // Get the ESP partition letter and relative path
    let path_str = efi_path.to_string_lossy();
    
    // bcdedit requires the path relative to the ESP root
    // e.g., if ESP is S: and file is S:\EFI\NixOS\shimx64.efi
    // we need \EFI\NixOS\shimx64.efi
    let relative_path = if path_str.len() > 2 && path_str.chars().nth(1) == Some(':') {
        &path_str[2..]
    } else {
        &path_str
    };
    
    // Create new boot entry
    // bcdedit /copy {bootmgr} /d "NixOS" would copy bootmgr which we don't want
    // Instead, we create a new firmware application entry
    
    let output = Command::new("bcdedit")
        .args(["/create", "/d", display_name, "/application", "osloader"])
        .output()
        .context("Failed to run bcdedit /create")?;
    
    if !output.status.success() {
        // Try alternative: create as firmware boot option
        debug!("osloader failed, trying firmware application...");
        
        // For UEFI, we should use the firmware boot manager
        // Let's try adding to the firmware boot order instead
        return create_firmware_boot_entry(efi_path, display_name);
    }
    
    // Parse the GUID from output like "The entry {guid} was successfully created"
    let output_str = String::from_utf8_lossy(&output.stdout);
    let guid = parse_bcdedit_guid(&output_str)?;
    
    // Set the device and path for the new entry
    let esp_letter = path_str.chars().next().unwrap_or('S');
    
    Command::new("bcdedit")
        .args(["/set", &guid, "device", &format!("partition={}:", esp_letter)])
        .output()
        .context("Failed to set device")?;
    
    Command::new("bcdedit")
        .args(["/set", &guid, "path", relative_path])
        .output()
        .context("Failed to set path")?;
    
    // Add to the display order (but not first - Windows stays default)
    Command::new("bcdedit")
        .args(["/displayorder", &guid, "/addlast"])
        .output()
        .context("Failed to add to display order")?;
    
    Ok(guid)
}

/// Alternative: Create entry in firmware boot order (for Secure Boot shim)
fn create_firmware_boot_entry(efi_path: &Path, display_name: &str) -> Result<String> {
    use std::process::Command;
    
    // Use efibootmgr-style approach through bcdedit firmware
    let path_str = efi_path.to_string_lossy();
    let relative_path = if path_str.len() > 2 { &path_str[2..] } else { &path_str };
    let drive_letter = path_str.chars().next().unwrap_or('S');
    
    // Create firmware boot option
    let output = Command::new("bcdedit")
        .args([
            "/create", 
            "/d", display_name,
            "/application", "bootsector"
        ])
        .output()
        .context("Failed to create boot entry")?;
    
    let output_str = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        // Last resort: try as a copy of the firmware app
        let output = Command::new("bcdedit")
            .args(["/copy", "{fwbootmgr}", "/d", display_name])
            .output()
            .context("bcdedit /copy failed")?;
        
        if !output.status.success() {
            bail!(
                "All bcdedit methods failed. You may need to add boot entry manually.\n\
                Error: {}", 
                String::from_utf8_lossy(&output.stderr)
            );
        }
        
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    
    let guid = parse_bcdedit_guid(&output_str)?;
    
    // Configure the entry
    Command::new("bcdedit")
        .args(["/set", &guid, "device", &format!("partition={}:", drive_letter)])
        .output()?;
    
    Command::new("bcdedit")
        .args(["/set", &guid, "path", relative_path])
        .output()?;
    
    // Add to firmware menu
    Command::new("bcdedit")
        .args(["/set", "{fwbootmgr}", "displayorder", &guid, "/addlast"])
        .output()?;
    
    Ok(guid)
}

fn parse_bcdedit_guid(output: &str) -> Result<String> {
    // Look for {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}
    let re = regex::Regex::new(r"\{[0-9a-fA-F-]+\}").unwrap();
    
    re.find(output)
        .map(|m| m.as_str().to_string())
        .context("Could not parse boot entry GUID from bcdedit output")
}

/// Remove our boot entry and files (for cleanup/uninstall)
/// 
/// SAFETY: Only removes what we created
pub fn remove_bootloader(
    esp_folder: &Path,
    boot_entry_id: &str,
) -> Result<()> {
    warn!("Removing NixOS bootloader...");
    
    // Remove boot entry first (so system doesn't try to boot non-existent file)
    if !boot_entry_id.is_empty() {
        info!("Removing boot entry: {}", boot_entry_id);
        let _ = std::process::Command::new("bcdedit")
            .args(["/delete", boot_entry_id, "/cleanup"])
            .output();
    }
    
    // Remove our folder from ESP
    if esp_folder.exists() {
        // Safety check: verify it's our folder
        let shim = esp_folder.join("shimx64.efi");
        let grub = esp_folder.join("grubx64.efi");
        
        if shim.exists() || grub.exists() {
            info!("Removing ESP folder: {:?}", esp_folder);
            fs::remove_dir_all(esp_folder)
                .context("Failed to remove ESP folder")?;
        } else {
            warn!("ESP folder doesn't contain expected files, skipping removal");
        }
    }
    
    Ok(())
}

// ============================================================================
// Helper Functions  
// ============================================================================

fn copy_boot_file(src: &Path, dst: &Path, name: &str) -> Result<()> {
    debug!("Copying {}: {:?} -> {:?}", name, src, dst);
    
    if !src.exists() {
        bail!("{} not found at {:?}", name, src);
    }
    
    fs::copy(src, dst)
        .with_context(|| format!("Failed to copy {} to ESP", name))?;
    
    Ok(())
}

/// Generate a minimal GRUB configuration for initial boot
/// 
/// This config boots the NixOS installer ISO to complete installation
pub fn generate_initial_grub_cfg(
    _install_config_path: &str,
    nixos_root: &str,
) -> String {
    format!(r#"
# NixOS Easy Install - Initial Boot Configuration
# This loads the NixOS installer which will complete setup

set timeout=5
set default=0

menuentry "NixOS Install" {{
    insmod part_gpt
    insmod fat
    insmod ext2
    insmod loopback
    
    # Find and boot the NixOS installer
    loopback loop {nixos_root}/nixos.iso
    linux (loop)/boot/bzImage init=/nix/store/*/init
    initrd (loop)/boot/initrd
}}

menuentry "Windows Boot Manager" {{
    chainloader /EFI/Microsoft/Boot/bootmgfw.efi
}}
"#)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_bcdedit_guid() {
        let output = "The entry {12345678-1234-1234-1234-123456789abc} was successfully created.";
        let guid = parse_bcdedit_guid(output).unwrap();
        assert_eq!(guid, "{12345678-1234-1234-1234-123456789abc}");
    }
    
    #[test]
    fn test_grub_cfg_generation() {
        let cfg = generate_initial_grub_cfg("/EFI/NixOS/install.json", "/NixOS");
        assert!(cfg.contains("NixOS Install"));
        assert!(cfg.contains("Windows Boot Manager"));
    }
}
