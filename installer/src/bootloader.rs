//! Bootloader setup for UEFI systems
//! 
//! This module handles:
//! 1. Copying boot files to ESP (shimaa64.efi/grubaa64.efi for ARM64, shimx64.efi/grubx64.efi for x86_64)
//! 2. Creating UEFI boot entry via direct NVRAM writes (for ARM64) or bcdedit (x86)
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
    
    /// NixOS installer kernel (optional, downloaded from release)
    pub kernel: Option<PathBuf>,
    
    /// NixOS installer initrd (optional, downloaded from release)
    pub initrd: Option<PathBuf>,
    
    /// Device Tree Blob for ARM64 devices (optional, X1E only)
    pub device_dtb: Option<PathBuf>,
    
    /// Architecture (x86_64 or aarch64)
    pub arch: String,
}

/// Result of bootloader setup
#[derive(Debug, Clone)]
pub struct BootloaderSetupResult {
    /// Path to our boot folder on ESP
    pub esp_folder: PathBuf,
    
    /// Path to boot files (kernel/initrd) - may be different from ESP for loopback
    pub boot_files_folder: PathBuf,
    
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
    
    // Check 1: ESP has enough space (use centralized requirement from assets module)
    let required_space = crate::assets::required_esp_space();
    if esp.free_space < required_space {
        errors.push(format!(
            "ESP has insufficient space: {} available, {}MB required",
            crate::system::format_bytes(esp.free_space),
            crate::assets::required_esp_space_mb()
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
/// For loopback installs, large boot files (kernel, initrd, dtb) are stored
/// in `boot_files_dir` (typically C:\NixOS\boot) instead of ESP to avoid
/// space limitations. The ESP only needs ~10MB for GRUB and shim.
/// 
/// SAFETY:
/// - Creates new folder in EFI directory only
/// - Does not modify any existing boot entries
/// - All files are additive
pub fn setup_bootloader(
    esp: &EspInfo, 
    boot_files: &BootFiles,
    display_name: &str,
    boot_files_dir: Option<&Path>,
) -> Result<BootloaderSetupResult> {
    info!("Setting up {} bootloader on ESP at {:?}", boot_files.arch, esp.mount_point);
    
    // Validate architecture matches detected system
    let detected_arch = crate::assets::detect_arch();
    let boot_arch = if boot_files.arch.starts_with("aarch64") { "aarch64" } else { &boot_files.arch };
    if detected_arch != boot_arch {
        bail!(
            "Architecture mismatch: boot files are for {} but system detected as {}",
            boot_files.arch, detected_arch
        );
    }
    
    // Run preflight
    let preflight = preflight_check(esp)?;
    if !preflight.passed {
        bail!("Bootloader preflight failed: {:?}", preflight.errors);
    }
    
    let nixos_folder = preflight.nixos_folder;
    
    // Create our boot folder on ESP
    info!("Creating NixOS boot folder: {:?}", nixos_folder);
    fs::create_dir_all(&nixos_folder)
        .context("Failed to create NixOS boot folder")?;
    
    // Determine where to store large boot files (kernel, initrd, dtb)
    // For loopback installs, use the provided directory (on NTFS) to avoid ESP space issues
    let large_files_folder = if let Some(dir) = boot_files_dir {
        let boot_dir = dir.join("boot");
        info!("Using NTFS partition for large boot files: {:?}", boot_dir);
        fs::create_dir_all(&boot_dir)
            .context("Failed to create boot files directory")?;
        boot_dir
    } else {
        nixos_folder.clone()
    };
    
    // Determine filenames based on architecture
    let (shim_name, grub_name) = if boot_files.arch == "aarch64" || boot_files.arch.starts_with("aarch64") {
        ("shimaa64.efi", "grubaa64.efi")
    } else {
        ("shimx64.efi", "grubx64.efi")
    };
    
    // Copy EFI boot files (always go to ESP - these are small)
    copy_boot_file(&boot_files.shim, &nixos_folder.join(shim_name), "shim")?;
    copy_boot_file(&boot_files.grub, &nixos_folder.join(grub_name), "GRUB")?;
    copy_boot_file(&boot_files.mok_cert, &nixos_folder.join("MOK.cer"), "MOK certificate")?;
    copy_boot_file(&boot_files.grub_cfg, &nixos_folder.join("grub.cfg"), "GRUB config")?;
    
    // Copy NixOS installer kernel and initrd to the large files folder
    // These can be 500MB+ and won't fit on most ESPs
    if let Some(kernel) = &boot_files.kernel {
        copy_boot_file(kernel, &large_files_folder.join("bzImage"), "NixOS kernel")?;
    }
    if let Some(initrd) = &boot_files.initrd {
        copy_boot_file(initrd, &large_files_folder.join("initrd"), "NixOS initrd")?;
    }
    // Copy Device Tree Blob for ARM64 X1E (if provided)
    if let Some(dtb) = &boot_files.device_dtb {
        let dtb_dest = large_files_folder.join("device.dtb");
        copy_boot_file(dtb, &dtb_dest, "Device Tree Blob")?;
        // Verify DTB was actually copied (critical for X1E boot)
        if !dtb_dest.exists() {
            bail!("Failed to copy Device Tree Blob - file not found after copy");
        }
        let dtb_size = std::fs::metadata(&dtb_dest)?.len();
        if dtb_size == 0 {
            bail!("Device Tree Blob copied is empty (0 bytes)");
        }
        info!("Device Tree Blob copied successfully ({} bytes)", dtb_size);
    }
    
    // Create UEFI boot entry
    info!("Creating UEFI boot entry...");
    
    // Try bcdedit first (more reliable on most systems)
    // Then fall back to direct NVRAM writes if bcdedit fails
    let boot_entry_id = match create_boot_entry_bcdedit(esp, &nixos_folder.join(shim_name), display_name) {
        Ok(id) => {
            info!("Created boot entry via bcdedit: {}", id);
            id
        }
        Err(e) => {
            warn!("bcdedit failed ({}), trying direct NVRAM write...", e);
            create_uefi_boot_entry_direct(esp, &nixos_folder.join(shim_name), display_name)?
        }
    };
    
    // Verify the boot entry was actually created
    let verified = verify_uefi_boot_entry(&boot_entry_id).unwrap_or_else(|e| {
        warn!("Could not verify boot entry: {}", e);
        // Assume success if we can't verify - the create call succeeded
        true
    });
    
    if !verified {
        // Try to clean up the files we copied
        warn!("Boot entry creation could not be verified, cleaning up...");
        let _ = fs::remove_dir_all(&nixos_folder);
        bail!("Boot entry creation failed - entry {} not found", boot_entry_id);
    }
    
    info!("Bootloader setup complete. Entry ID: {}", boot_entry_id);
    
    Ok(BootloaderSetupResult {
        esp_folder: nixos_folder,
        boot_files_folder: large_files_folder,
        boot_entry_id,
        secure_boot_ready: true,
    })
}

/// Verify that a UEFI boot entry exists
/// 
/// For Boot#### entries (from direct NVRAM), we check BootOrder.
/// For {guid} entries (from bcdedit), we check bcdedit /enum firmware.
#[cfg(target_os = "windows")]
fn verify_uefi_boot_entry(entry_id: &str) -> Result<bool> {
    // bcdedit GUID format: {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}
    if entry_id.starts_with('{') && entry_id.ends_with('}') {
        // Verify via bcdedit /enum firmware
        use std::process::Command;
        let output = Command::new("bcdedit")
            .args(["/enum", "firmware"])
            .output()
            .context("Failed to run bcdedit /enum firmware")?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.contains(entry_id))
    } else if entry_id.starts_with("Boot") {
        // NVRAM Boot#### format - check BootOrder
        let num_str = &entry_id[4..];
        let entry_num: u16 = u16::from_str_radix(num_str, 16)
            .context("Invalid boot entry ID format")?;
        
        let boot_order = read_boot_order()?;
        Ok(boot_order.contains(&entry_num))
    } else {
        // Unknown format
        warn!("Unknown boot entry ID format: {}", entry_id);
        Ok(false)
    }
}

#[cfg(not(target_os = "windows"))]
fn verify_uefi_boot_entry(_entry_id: &str) -> Result<bool> {
    // Can't verify on non-Windows
    Ok(true)
}

/// Create UEFI boot entry by writing directly to NVRAM variables
/// 
/// This is required on ARM64 devices where bcdedit doesn't create proper
/// UEFI firmware boot entries. It uses the Windows SetFirmwareEnvironmentVariable API
/// to write Boot#### and BootOrder UEFI variables directly.
/// 
/// Based on the approach used by efibootmgr on Linux.
#[cfg(windows)]
fn create_uefi_boot_entry_direct(esp: &EspInfo, efi_path: &Path, description: &str) -> Result<String> {
    // EFI Global Variable GUID: 8BE4DF61-93CA-11D2-AA0D-00E098032B8C
    const EFI_GLOBAL_GUID: &str = "{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";
    
    // LOAD_OPTION attributes
    const LOAD_OPTION_ACTIVE: u32 = 0x00000001;
    
    // First, we need SE_SYSTEM_ENVIRONMENT_NAME privilege
    enable_firmware_privilege()?;
    
    // Get current BootOrder to find next available Boot#### number
    let boot_order = read_boot_order()?;
    let boot_num = find_next_boot_number(&boot_order);
    let boot_var_name = format!("Boot{:04X}", boot_num);
    
    info!("Creating UEFI boot entry {} via direct NVRAM write", boot_var_name);
    debug!("Current BootOrder: {:04X?}", boot_order);
    
    // Get the ESP partition GUID and number for the device path
    let (esp_guid, partition_num) = get_partition_info(&esp.mount_point)?;
    info!("ESP partition: GUID={}, number={}", esp_guid, partition_num);
    
    // Build the EFI device path to the shim
    let efi_relative = efi_path
        .strip_prefix(&esp.mount_point)
        .unwrap_or(efi_path);
    let efi_path_str = efi_relative.to_string_lossy().replace('/', "\\");
    
    // Build EFI_LOAD_OPTION structure
    let load_option = build_efi_load_option(
        LOAD_OPTION_ACTIVE,
        description,
        &esp_guid,
        &efi_path_str,
        partition_num,
    )?;
    
    // Write Boot#### variable
    write_uefi_variable(&boot_var_name, EFI_GLOBAL_GUID, &load_option)?;
    
    // Update BootOrder to include our new entry at the BEGINNING (so NixOS boots by default)
    let mut new_boot_order = vec![boot_num];
    new_boot_order.extend(boot_order.iter().cloned());
    if let Err(e) = write_boot_order(&new_boot_order) {
        // Rollback: delete the Boot#### we just created to avoid orphaned entry
        warn!("Failed to update BootOrder, rolling back Boot#### creation: {}", e);
        if let Err(del_err) = delete_uefi_variable(&boot_var_name, EFI_GLOBAL_GUID) {
            warn!("Could not delete orphaned boot entry {}: {}", boot_var_name, del_err);
        }
        return Err(e);
    }
    
    info!("Successfully created UEFI boot entry {} (now default boot option)", boot_var_name);
    
    // Log current boot order for debugging
    if let Ok(final_order) = read_boot_order() {
        info!("Final BootOrder: {:04X?}", final_order);
    }
    
    Ok(boot_var_name)
}

#[cfg(not(windows))]
fn create_uefi_boot_entry_direct(_esp: &EspInfo, _efi_path: &Path, _description: &str) -> Result<String> {
    bail!("Direct UEFI boot entry creation is only supported on Windows")
}

/// Enable SE_SYSTEM_ENVIRONMENT_NAME privilege required for UEFI variable access
#[cfg(windows)]
fn enable_firmware_privilege() -> Result<()> {
    use windows::Win32::Foundation::{HANDLE, LUID, CloseHandle};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, 
        TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY,
        TOKEN_PRIVILEGES, SE_PRIVILEGE_ENABLED, LUID_AND_ATTRIBUTES,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::w;
    
    unsafe {
        let mut token: HANDLE = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
            .context("Failed to open process token")?;
        
        let mut luid = LUID::default();
        // SE_SYSTEM_ENVIRONMENT_NAME = "SeSystemEnvironmentPrivilege"
        LookupPrivilegeValueW(None, w!("SeSystemEnvironmentPrivilege"), &mut luid)
            .context("Failed to lookup firmware privilege")?;
        
        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }; 1],
        };
        
        let result = AdjustTokenPrivileges(
            token,
            false,
            Some(&tp),
            0,
            None,
            None,
        );
        
        let _ = CloseHandle(token);
        
        result.context("Failed to enable firmware privilege")?;
    }
    
    Ok(())
}

/// Read current BootOrder UEFI variable
#[cfg(windows)]
fn read_boot_order() -> Result<Vec<u16>> {
    use windows::Win32::System::WindowsProgramming::GetFirmwareEnvironmentVariableW;
    use windows::core::PCWSTR;
    
    const EFI_GLOBAL_GUID: &str = "{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";
    
    let var_name: Vec<u16> = "BootOrder\0".encode_utf16().collect();
    let guid_name: Vec<u16> = EFI_GLOBAL_GUID.encode_utf16().chain(std::iter::once(0)).collect();
    
    // BootOrder is typically small (max ~128 entries = 256 bytes) but allocate more to be safe
    // UEFI spec doesn't define max, but systems rarely have more than a few dozen boot entries
    let mut buffer = vec![0u8; 1024];
    
    let size = unsafe {
        GetFirmwareEnvironmentVariableW(
            PCWSTR(var_name.as_ptr()),
            PCWSTR(guid_name.as_ptr()),
            Some(buffer.as_mut_ptr() as *mut _),
            buffer.len() as u32,
        )
    };
    
    if size == 0 {
        // No BootOrder exists, return empty
        debug!("No existing BootOrder found");
        return Ok(Vec::new());
    }
    
    // Check if we hit the buffer limit (unlikely but possible)
    if size as usize >= buffer.len() {
        warn!("BootOrder may have been truncated ({} bytes) - system has many boot entries", size);
    }
    
    // Validate BootOrder size is even (array of u16)
    if size % 2 != 0 {
        warn!("BootOrder has odd size {} bytes, may be corrupted", size);
    }
    
    // BootOrder is an array of u16 boot entry numbers
    let mut boot_order = Vec::new();
    for i in (0..size as usize).step_by(2) {
        if i + 1 < size as usize {
            let entry = u16::from_le_bytes([buffer[i], buffer[i + 1]]);
            boot_order.push(entry);
        }
    }
    
    debug!("Current BootOrder: {:04X?}", boot_order);
    Ok(boot_order)
}

/// Find the next available Boot#### number
fn find_next_boot_number(boot_order: &[u16]) -> u16 {
    // Start from 0x0010 to avoid conflicts with Windows entries
    let mut num = 0x0010u16;
    while boot_order.contains(&num) {
        num += 1;
    }
    num
}

/// Write BootOrder UEFI variable
#[cfg(windows)]
fn write_boot_order(boot_order: &[u16]) -> Result<()> {
    use windows::Win32::System::WindowsProgramming::SetFirmwareEnvironmentVariableExW;
    use windows::core::PCWSTR;
    
    const EFI_GLOBAL_GUID: &str = "{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";
    // VARIABLE_ATTRIBUTE_NON_VOLATILE | VARIABLE_ATTRIBUTE_BOOTSERVICE_ACCESS | VARIABLE_ATTRIBUTE_RUNTIME_ACCESS
    const ATTRS: u32 = 0x07;
    
    let var_name: Vec<u16> = "BootOrder\0".encode_utf16().collect();
    let guid_name: Vec<u16> = EFI_GLOBAL_GUID.encode_utf16().chain(std::iter::once(0)).collect();
    
    // Convert boot_order to bytes
    let mut data = Vec::new();
    for &num in boot_order {
        data.extend_from_slice(&num.to_le_bytes());
    }
    
    let result = unsafe {
        SetFirmwareEnvironmentVariableExW(
            PCWSTR(var_name.as_ptr()),
            PCWSTR(guid_name.as_ptr()),
            Some(data.as_ptr() as *const _),
            data.len() as u32,
            ATTRS,
        )
    };
    
    result.context("Failed to write BootOrder")?;
    info!("Updated BootOrder: {:04X?}", boot_order);
    Ok(())
}

/// Write a UEFI variable
#[cfg(windows)]
fn write_uefi_variable(name: &str, guid: &str, data: &[u8]) -> Result<()> {
    use windows::Win32::System::WindowsProgramming::SetFirmwareEnvironmentVariableExW;
    use windows::core::PCWSTR;
    
    // VARIABLE_ATTRIBUTE_NON_VOLATILE | VARIABLE_ATTRIBUTE_BOOTSERVICE_ACCESS | VARIABLE_ATTRIBUTE_RUNTIME_ACCESS
    const ATTRS: u32 = 0x07;
    
    let var_name: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
    let guid_name: Vec<u16> = format!("{}\0", guid).encode_utf16().collect();
    
    info!("Writing UEFI variable {} ({} bytes)", name, data.len());
    
    let result = unsafe {
        SetFirmwareEnvironmentVariableExW(
            PCWSTR(var_name.as_ptr()),
            PCWSTR(guid_name.as_ptr()),
            Some(data.as_ptr() as *const _),
            data.len() as u32,
            ATTRS,
        )
    };
    
    if let Err(e) = result.as_ref() {
        warn!("SetFirmwareEnvironmentVariableExW failed for {}: {:?}", name, e);
        return Err(anyhow::anyhow!("Failed to write UEFI variable {}: {:?}", name, e));
    }
    
    // Verify write by reading back
    match read_uefi_variable(name, guid) {
        Ok(read_data) => {
            if read_data.len() != data.len() {
                warn!("UEFI variable {} size mismatch: wrote {} bytes, read {} bytes", 
                      name, data.len(), read_data.len());
            } else {
                info!("Verified UEFI variable {} ({} bytes)", name, read_data.len());
            }
        }
        Err(e) => {
            warn!("Could not verify UEFI variable {} after write: {}", name, e);
        }
    }
    
    Ok(())
}

/// Read a UEFI variable
#[cfg(windows)]
fn read_uefi_variable(name: &str, guid: &str) -> Result<Vec<u8>> {
    use windows::Win32::System::WindowsProgramming::GetFirmwareEnvironmentVariableW;
    use windows::core::PCWSTR;
    
    let var_name: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
    let guid_name: Vec<u16> = format!("{}\0", guid).encode_utf16().collect();
    
    let mut buffer = vec![0u8; 4096];
    
    let size = unsafe {
        GetFirmwareEnvironmentVariableW(
            PCWSTR(var_name.as_ptr()),
            PCWSTR(guid_name.as_ptr()),
            Some(buffer.as_mut_ptr() as *mut _),
            buffer.len() as u32,
        )
    };
    
    if size == 0 {
        bail!("Failed to read UEFI variable {}", name);
    }
    
    buffer.truncate(size as usize);
    Ok(buffer)
}

/// Delete a UEFI variable (used for rollback)
#[cfg(windows)]
fn delete_uefi_variable(name: &str, guid: &str) -> Result<()> {
    use windows::Win32::System::WindowsProgramming::SetFirmwareEnvironmentVariableExW;
    use windows::core::PCWSTR;
    
    let var_name: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
    let guid_name: Vec<u16> = format!("{}\0", guid).encode_utf16().collect();
    
    // Writing with size 0 deletes the variable
    let result = unsafe {
        SetFirmwareEnvironmentVariableExW(
            PCWSTR(var_name.as_ptr()),
            PCWSTR(guid_name.as_ptr()),
            None,
            0,
            0,
        )
    };
    
    result.context(format!("Failed to delete UEFI variable {}", name))?;
    debug!("Deleted UEFI variable {}", name);
    Ok(())
}

/// Get the GPT partition GUID and number for a mounted volume
#[cfg(windows)]
fn get_partition_info(mount_point: &Path) -> Result<(String, u32)> {
    use std::process::Command;
    
    // Use PowerShell to get partition GUID and number
    let drive_letter = mount_point.to_string_lossy();
    let letter = drive_letter.chars().next().unwrap_or('S');
    let script = format!(
        r#"$part = Get-Partition -DriveLetter '{}' -ErrorAction SilentlyContinue; 
           if ($part) {{ 
               Write-Output "$($part.Guid)|$($part.PartitionNumber)"
           }}"#,
        letter
    );
    
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .context("Failed to run PowerShell")?;
    
    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    
    if result.contains('|') {
        let parts: Vec<&str> = result.split('|').collect();
        if parts.len() >= 2 {
            let guid = parts[0].trim();
            let part_num_str = parts[1].trim();
            
            // Parse partition number with error handling
            let part_num: u32 = part_num_str.parse()
                .context(format!("Invalid partition number '{}' from PowerShell", part_num_str))?;
            
            if part_num == 0 {
                bail!("Invalid partition number 0 - ESP partition detection failed");
            }
            
            let formatted_guid = if guid.starts_with('{') { 
                guid.to_string() 
            } else { 
                format!("{{{}}}", guid) 
            };
            
            info!("ESP partition GUID: {}, Number: {}", formatted_guid, part_num);
            return Ok((formatted_guid, part_num));
        }
    }
    
    // PowerShell detection failed - this is more dangerous, warn loudly
    warn!("PowerShell partition detection failed. Falling back to mountvol + partition 1.");
    warn!("If your ESP is not on partition 1, the boot entry will be incorrect!");
    
    // Fallback: try to get GUID from mountvol, assume partition 1
    let mv_output = Command::new("mountvol")
        .arg(mount_point)
        .arg("/L")
        .output()?;
    let mv_str = String::from_utf8_lossy(&mv_output.stdout);
    // Parse volume GUID from something like \\?\Volume{guid}\
    if let Some(start) = mv_str.find('{') {
        if let Some(end) = mv_str.find('}') {
            warn!("Using partition 1 as fallback - verify this is correct for your system");
            return Ok((mv_str[start..=end].to_string(), 1));
        }
    }
    
    bail!("Could not determine ESP partition GUID")
}

/// Build an EFI_LOAD_OPTION structure
/// 
/// Structure:
///   UINT32 Attributes
///   UINT16 FilePathListLength
///   CHAR16[] Description (null-terminated)
///   EFI_DEVICE_PATH_PROTOCOL[] FilePathList
///   UINT8[] OptionalData (optional)
#[cfg(windows)]
fn build_efi_load_option(
    attributes: u32,
    description: &str,
    partition_guid: &str,
    file_path: &str,
    partition_num: u32,
) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    
    // Attributes (4 bytes, little-endian)
    data.extend_from_slice(&attributes.to_le_bytes());
    
    // We'll fill in FilePathListLength later (need to know device path size)
    let file_path_len_offset = data.len();
    data.extend_from_slice(&0u16.to_le_bytes()); // placeholder
    
    // Description as UTF-16LE null-terminated
    for c in description.encode_utf16() {
        data.extend_from_slice(&c.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes()); // null terminator
    
    // Build EFI device path
    let device_path = build_efi_device_path(partition_guid, file_path, partition_num)?;
    
    // Update FilePathListLength
    let path_len = device_path.len() as u16;
    data[file_path_len_offset..file_path_len_offset + 2].copy_from_slice(&path_len.to_le_bytes());
    
    // Append device path
    data.extend_from_slice(&device_path);
    
    Ok(data)
}

/// Build an EFI device path for HD(partition)/File(path)
/// 
/// This creates:
/// 1. HD Device Path (type 4, subtype 1) - hard drive partition
/// 2. File Path (type 4, subtype 4) - file path within partition  
/// 3. End Device Path (type 0x7F, subtype 0xFF)
#[cfg(windows)]
fn build_efi_device_path(partition_guid: &str, file_path: &str, partition_num: u32) -> Result<Vec<u8>> {
    let mut path = Vec::new();
    
    // Parse partition GUID (remove braces)
    let guid_str = partition_guid.trim_matches(|c| c == '{' || c == '}');
    let guid_bytes = parse_guid_to_bytes(guid_str)?;
    
    // HD Device Path Node (Media Device Path)
    // Type: 0x04 (Media Device Path)
    // Subtype: 0x01 (Hard Drive)
    // Length: 42 bytes
    let hd_node = build_hd_device_path_node(&guid_bytes, partition_num)?;
    path.extend_from_slice(&hd_node);
    
    // File Path Device Path Node
    // Type: 0x04 (Media Device Path)
    // Subtype: 0x04 (File Path)
    let file_node = build_file_path_node(file_path);
    path.extend_from_slice(&file_node);
    
    // End Device Path Node
    // Type: 0x7F (End of Hardware Device Path)
    // Subtype: 0xFF (End Entire Device Path)
    // Length: 4
    path.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]);
    
    Ok(path)
}

/// Build HD Device Path node for GPT partition
#[cfg(windows)]
fn build_hd_device_path_node(partition_guid: &[u8; 16], partition_num: u32) -> Result<Vec<u8>> {
    let mut node = Vec::new();
    
    // Type: Media Device Path (0x04)
    node.push(0x04);
    // Subtype: Hard Drive (0x01)
    node.push(0x01);
    // Length: 42 bytes (little-endian)
    node.extend_from_slice(&42u16.to_le_bytes());
    
    // Partition Number (4 bytes) - actual partition number from system
    node.extend_from_slice(&partition_num.to_le_bytes());
    // Partition Start (8 bytes) - 0 means "don't care, use GUID"
    node.extend_from_slice(&0u64.to_le_bytes());
    // Partition Size (8 bytes) - 0 means "don't care"
    node.extend_from_slice(&0u64.to_le_bytes());
    // Partition Signature (16 bytes) - GPT GUID
    node.extend_from_slice(partition_guid);
    // Partition Format: GPT (0x02)
    node.push(0x02);
    // Signature Type: GUID (0x02)
    node.push(0x02);
    
    assert_eq!(node.len(), 42);
    Ok(node)
}

/// Build File Path device path node
#[cfg(windows)]
fn build_file_path_node(file_path: &str) -> Vec<u8> {
    let mut node = Vec::new();
    
    // Ensure path starts with backslash and uses backslashes
    let normalized = if file_path.starts_with('\\') {
        file_path.to_string()
    } else {
        format!("\\{}", file_path)
    };
    let normalized = normalized.replace('/', "\\");
    
    // Convert to UTF-16LE including null terminator
    let mut path_utf16: Vec<u8> = Vec::new();
    for c in normalized.encode_utf16() {
        path_utf16.extend_from_slice(&c.to_le_bytes());
    }
    path_utf16.extend_from_slice(&0u16.to_le_bytes()); // null terminator
    
    // Type: Media Device Path (0x04)
    node.push(0x04);
    // Subtype: File Path (0x04)
    node.push(0x04);
    // Length: header (4) + path data
    let len = (4 + path_utf16.len()) as u16;
    node.extend_from_slice(&len.to_le_bytes());
    // Path data
    node.extend_from_slice(&path_utf16);
    
    node
}

/// Parse a GUID string to bytes in EFI mixed-endian format
/// Input: "8BE4DF61-93CA-11D2-AA0D-00E098032B8C"
/// Output: 16 bytes in EFI format (first 3 components little-endian, rest big-endian)
fn parse_guid_to_bytes(guid: &str) -> Result<[u8; 16]> {
    let parts: Vec<&str> = guid.split('-').collect();
    if parts.len() != 5 {
        bail!("Invalid GUID format: {}", guid);
    }
    
    let mut bytes = [0u8; 16];
    
    // Part 1: 4 bytes, little-endian
    let p1 = u32::from_str_radix(parts[0], 16).context("Invalid GUID part 1")?;
    bytes[0..4].copy_from_slice(&p1.to_le_bytes());
    
    // Part 2: 2 bytes, little-endian
    let p2 = u16::from_str_radix(parts[1], 16).context("Invalid GUID part 2")?;
    bytes[4..6].copy_from_slice(&p2.to_le_bytes());
    
    // Part 3: 2 bytes, little-endian
    let p3 = u16::from_str_radix(parts[2], 16).context("Invalid GUID part 3")?;
    bytes[6..8].copy_from_slice(&p3.to_le_bytes());
    
    // Part 4: 2 bytes, big-endian (as-is)
    let p4 = u16::from_str_radix(parts[3], 16).context("Invalid GUID part 4")?;
    bytes[8..10].copy_from_slice(&p4.to_be_bytes());
    
    // Part 5: 6 bytes, big-endian (as-is)
    let p5 = u64::from_str_radix(parts[4], 16).context("Invalid GUID part 5")?;
    bytes[10..16].copy_from_slice(&p5.to_be_bytes()[2..8]);
    
    Ok(bytes)
}

fn parse_bcdedit_guid(output: &str) -> Result<String> {
    // Look for {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}
    // This regex pattern is known-good at compile time, so unwrap is safe
    let re = regex::Regex::new(r"\{[0-9a-fA-F-]+\}")
        .expect("valid regex pattern");
    
    re.find(output)
        .map(|m| m.as_str().to_string())
        .context("Could not parse boot entry GUID from bcdedit output")
}

/// Try to create a firmware application entry using bcdedit /create
#[cfg(windows)]
fn try_create_firmware_entry(description: &str) -> Result<String> {
    use std::process::Command;
    
    // Use /application osloader which creates an EFI application entry
    // that can load any EFI binary (including shim)
    let output = Command::new("bcdedit")
        .args(["/create", "/d", description, "/application", "osloader"])
        .output()
        .context("Failed to run bcdedit /create")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("bcdedit /create failed: {}", stderr);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_bcdedit_guid(&stdout)
}

/// Try to copy {fwbootmgr} to create a firmware-level entry
#[cfg(windows)]
fn try_copy_fwbootmgr(description: &str) -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("bcdedit")
        .args(["/copy", "{fwbootmgr}", "/d", description])
        .output()
        .context("Failed to run bcdedit /copy {fwbootmgr}")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("bcdedit /copy {{fwbootmgr}} failed: {}", stderr);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_bcdedit_guid(&stdout)
}

/// Try to copy {bootmgr} as a last resort
#[cfg(windows)]
fn try_copy_bootmgr(description: &str) -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("bcdedit")
        .args(["/copy", "{bootmgr}", "/d", description])
        .output()
        .context("Failed to run bcdedit /copy")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("bcdedit /copy {{bootmgr}} failed: {}", stderr);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_bcdedit_guid(&stdout)
}

/// Create UEFI boot entry using bcdedit
/// 
/// This creates a firmware-level boot entry by:
/// 1. Creating a new firmware application entry in the BCD firmware store
/// 2. Setting the path to our shim EFI file
/// 3. Adding the entry to the firmware boot order ({fwbootmgr})
/// 
/// IMPORTANT: We must create a FIRMWARE APPLICATION entry, not a Windows loader entry.
/// Copying {bootmgr} creates a Windows Boot Manager entry that tries to parse BCD data,
/// which fails for non-Windows EFI applications like shim/GRUB.
#[cfg(windows)]
fn create_boot_entry_bcdedit(esp: &EspInfo, efi_path: &Path, description: &str) -> Result<String> {
    use std::process::Command;
    
    info!("Creating UEFI firmware boot entry via bcdedit...");
    
    // Step 1: Create a new firmware application entry
    // Try multiple methods in order of preference:
    // 1. /create /application osloader - creates a proper EFI application entry
    // 2. /copy {fwbootmgr} - copies firmware boot manager (not Windows boot manager)
    // 3. /copy {bootmgr} as last resort (may have issues with shim)
    
    let guid = try_create_firmware_entry(description)
        .or_else(|e| {
            warn!("Primary create failed: {}, trying {{fwbootmgr}} copy...", e);
            try_copy_fwbootmgr(description)
        })
        .or_else(|e| {
            warn!("{{fwbootmgr}} copy failed: {}, trying {{bootmgr}} copy as last resort...", e);
            try_copy_bootmgr(description)
        })?;
    
    info!("Created new boot entry: {}", guid);
    
    // Step 2: Set the path to our shim
    let efi_relative = efi_path
        .strip_prefix(&esp.mount_point)
        .unwrap_or(efi_path);
    let efi_path_str = format!("\\{}", efi_relative.to_string_lossy().replace('/', "\\"));
    
    let path_output = Command::new("bcdedit")
        .args(["/set", &guid, "path", &efi_path_str])
        .output()
        .context("Failed to run bcdedit /set path")?;
    
    if !path_output.status.success() {
        // Cleanup: delete the entry we just created
        let _ = Command::new("bcdedit").args(["/delete", &guid]).output();
        let stderr = String::from_utf8_lossy(&path_output.stderr);
        bail!("bcdedit /set path failed: {}", stderr);
    }
    info!("Set boot path: {}", efi_path_str);
    
    // Step 3: Set the device to the ESP partition
    let esp_letter = esp.mount_point.to_string_lossy();
    let device_str = format!("partition={}", esp_letter.trim_end_matches('\\'));
    
    let device_output = Command::new("bcdedit")
        .args(["/set", &guid, "device", &device_str])
        .output()
        .context("Failed to run bcdedit /set device")?;
    
    if !device_output.status.success() {
        // Cleanup: delete the entry we just created
        let _ = Command::new("bcdedit").args(["/delete", &guid]).output();
        let stderr = String::from_utf8_lossy(&device_output.stderr);
        bail!("bcdedit /set device failed: {}", stderr);
    }
    info!("Set boot device: {}", device_str);
    
    // Step 4: Add to firmware boot order (at the beginning so NixOS boots by default)
    let order_output = Command::new("bcdedit")
        .args(["/set", "{fwbootmgr}", "displayorder", &guid, "/addfirst"])
        .output()
        .context("Failed to run bcdedit /set displayorder")?;
    
    if !order_output.status.success() {
        // Entry was created but not added to boot order - still usable but warn
        let stderr = String::from_utf8_lossy(&order_output.stderr);
        warn!("Could not add to boot order (entry still created): {}", stderr);
    } else {
        info!("Added {} to firmware boot order (first)", guid);
    }
    
    // Step 5: Clean up unnecessary inherited values from bootmgr
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "locale"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "inherit"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "default"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "resumeobject"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "displayorder"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "toolsdisplayorder"]).output();
    let _ = Command::new("bcdedit").args(["/deletevalue", &guid, "timeout"]).output();
    
    info!("Successfully created firmware boot entry {}", guid);
    Ok(guid)
}

#[cfg(not(windows))]
fn create_boot_entry_bcdedit(_esp: &EspInfo, _efi_path: &Path, _description: &str) -> Result<String> {
    bail!("bcdedit boot entry creation is only supported on Windows")
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
        // Safety check: verify it's our folder (check both x86_64 and ARM64 filenames)
        let shim_x64 = esp_folder.join("shimx64.efi");
        let shim_aa64 = esp_folder.join("shimaa64.efi");
        let grub_x64 = esp_folder.join("grubx64.efi");
        let grub_aa64 = esp_folder.join("grubaa64.efi");
        
        if shim_x64.exists() || shim_aa64.exists() || grub_x64.exists() || grub_aa64.exists() {
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
    
    let src_size = fs::metadata(src)?.len();
    
    fs::copy(src, dst)
        .with_context(|| format!("Failed to copy {} to ESP", name))?;
    
    // Verify the copy was successful
    let dst_size = fs::metadata(dst)?.len();
    if src_size != dst_size {
        bail!(
            "{} copy verification failed: source {} bytes, destination {} bytes",
            name, src_size, dst_size
        );
    }
    
    debug!("{} copied successfully ({} bytes)", name, dst_size);
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
