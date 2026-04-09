//! Installation logic - orchestrates the actual installation process
//! 
//! SAFETY DESIGN:
//! - Validates everything before making changes
//! - Each step is logged for debugging
//! - Provides rollback/cleanup on failure
//! - Progress reporting for UI feedback

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{info, warn, error};

use crate::config::InstallConfig;
use crate::system::{SystemInfo, EspInfo};
use crate::loopback::{LoopbackConfig, LoopbackPrepareResult};
use crate::bootloader::{BootFiles, BootloaderSetupResult};

/// Progress callback type
pub type ProgressCallback = Box<dyn Fn(f32, &str) + Send>;

/// State accumulated during installation (for rollback)
#[derive(Default)]
pub struct InstallState {
    /// Loopback files created (if any)
    pub loopback_result: Option<LoopbackPrepareResult>,
    
    /// Bootloader setup result (if any)
    pub bootloader_result: Option<BootloaderSetupResult>,
    
    /// ESP folder path for cleanup
    pub esp_folder: Option<PathBuf>,
}

/// Perform the installation
/// 
/// This orchestrates all installation steps in order, with rollback
/// capability if any step fails.
pub async fn install(
    config: InstallConfig, 
    system_info: &SystemInfo,
    progress: ProgressCallback,
) -> Result<()> {
    info!("Starting installation...");
    
    let mut state = InstallState::default();
    
    // Wrap installation in a function that handles cleanup on error
    let result = install_inner(&config, system_info, &progress, &mut state).await;
    
    if let Err(ref e) = result {
        error!("Installation failed: {}", e);
        progress(0.0, "Installation failed. Cleaning up...");
        
        // Attempt cleanup
        if let Err(cleanup_err) = cleanup(&state) {
            error!("Cleanup also failed: {}", cleanup_err);
        }
    }
    
    result
}

async fn install_inner(
    config: &InstallConfig,
    system_info: &SystemInfo,
    progress: &ProgressCallback,
    state: &mut InstallState,
) -> Result<()> {
    
    // Step 1: Final validation
    progress(0.05, "Validating system requirements...");
    validate_system(system_info)?;
    
    // Step 2: Prepare storage
    progress(0.10, "Preparing storage...");
    match config.install_type.as_str() {
        "loopback" | "quick" => {
            let result = prepare_loopback(config, progress).await?;
            state.loopback_result = Some(result);
        }
        "partition" | "full" => {
            prepare_partitions(config).await?;
        }
        _ => anyhow::bail!("Unknown install type: {}", config.install_type),
    }
    
    // Step 3: Download boot assets
    progress(0.30, "Downloading NixOS boot files...");
    let boot_files = download_boot_assets(config).await?;
    
    // Step 4: Set up bootloader
    progress(0.50, "Setting up bootloader...");
    let esp = system_info.esp.as_ref()
        .context("No EFI System Partition found")?;
    
    let bootloader_result = setup_bootloader(esp, &boot_files)?;
    state.esp_folder = Some(bootloader_result.esp_folder.clone());
    state.bootloader_result = Some(bootloader_result);
    
    // Step 5: Write install configuration
    progress(0.70, "Writing installation configuration...");
    write_install_config(config, esp)?;
    
    // Step 6: Final verification
    progress(0.90, "Verifying installation...");
    verify_setup(state)?;
    
    progress(1.0, "Ready to reboot!");
    
    Ok(())
}

fn validate_system(info: &SystemInfo) -> Result<()> {
    let validation = crate::system::validate_requirements(info);
    
    if !validation.passed {
        anyhow::bail!("System requirements not met: {}", validation.errors.join(", "));
    }
    
    if !info.is_uefi {
        warn!("System is not UEFI - legacy BIOS support is experimental");
    }
    
    Ok(())
}

async fn prepare_loopback(
    config: &InstallConfig, 
    progress: &ProgressCallback,
) -> Result<LoopbackPrepareResult> {
    let loopback_cfg = config.loopback.as_ref()
        .context("Loopback config missing")?;
    
    progress(0.12, "Creating NixOS directory...");
    
    let cfg = LoopbackConfig {
        target_dir: PathBuf::from(&loopback_cfg.target_dir),
        size_gb: loopback_cfg.size_gb,
        separate_home: false,
        home_size_gb: None,
    };
    
    // Run preflight
    let preflight = crate::loopback::preflight_check(&cfg)?;
    if !preflight.passed {
        anyhow::bail!("Loopback preflight failed: {}", preflight.errors.join(", "));
    }
    
    progress(0.15, "Creating disk image...");
    
    // Create the loopback files
    crate::loopback::prepare_loopback(&cfg)
}

async fn prepare_partitions(config: &InstallConfig) -> Result<()> {
    let partition = config.partition.as_ref()
        .context("Partition config missing")?;
    
    info!("Preparing partitions: root={}, boot={}", partition.root, partition.boot);
    
    // This is the dangerous path - not implemented yet
    // Would use diskpart to:
    // 1. Shrink Windows partition
    // 2. Create new GPT partition for NixOS
    // 3. Optionally create swap partition
    
    warn!("Full partition installation not yet implemented - use loopback for now");
    anyhow::bail!("Full partition installation coming soon. Please use Quick Install.")
}

async fn download_boot_assets(config: &InstallConfig) -> Result<BootFiles> {
    // TODO: Actually download these files
    // For now, we'll expect them to be bundled or point to paths
    
    // These would be:
    // - shimx64.efi from shim-signed (Microsoft-signed)
    // - grubx64.efi built/signed for this installer
    // - MOK certificate
    // - Initial GRUB config
    
    // Placeholder paths - these would be extracted from embedded resources
    // or downloaded from a known URL
    let temp_dir = std::env::temp_dir().join("nixos-install");
    std::fs::create_dir_all(&temp_dir)?;
    
    Ok(BootFiles {
        shim: temp_dir.join("shimx64.efi"),
        grub: temp_dir.join("grubx64.efi"),
        mok_cert: temp_dir.join("MOK.cer"),
        grub_cfg: temp_dir.join("grub.cfg"),
    })
}

fn setup_bootloader(esp: &EspInfo, boot_files: &BootFiles) -> Result<BootloaderSetupResult> {
    // First verify ESP is accessible
    if esp.mount_point.as_os_str().is_empty() {
        anyhow::bail!("ESP is not mounted. Please mount it and try again.");
    }
    
    crate::bootloader::setup_bootloader(esp, boot_files, "NixOS")
}

fn write_install_config(config: &InstallConfig, esp: &EspInfo) -> Result<()> {
    let config_json = config.to_json()?;
    
    // Write to ESP so the installer initrd can read it
    let config_dir = esp.mount_point.join("EFI").join("NixOS");
    std::fs::create_dir_all(&config_dir)?;
    
    let config_path = config_dir.join("install-config.json");
    std::fs::write(&config_path, &config_json)
        .context("Failed to write install config to ESP")?;
    
    info!("Wrote install config to {:?}", config_path);
    
    Ok(())
}

fn verify_setup(state: &InstallState) -> Result<()> {
    info!("Verifying installation setup...");
    
    // Check loopback files exist
    if let Some(ref loopback) = state.loopback_result {
        if !loopback.root_disk.exists() {
            anyhow::bail!("Root disk image not found at {:?}", loopback.root_disk);
        }
    }
    
    // Check bootloader files exist
    if let Some(ref bootloader) = state.bootloader_result {
        if !bootloader.esp_folder.exists() {
            anyhow::bail!("ESP folder not found at {:?}", bootloader.esp_folder);
        }
    }
    
    info!("Verification passed!");
    Ok(())
}

/// Clean up after failed installation
fn cleanup(state: &InstallState) -> Result<()> {
    warn!("Running cleanup after failed installation...");
    
    // Remove loopback files
    if let Some(ref loopback) = state.loopback_result {
        if let Some(parent) = loopback.root_disk.parent() {
            if let Err(e) = crate::loopback::cleanup_loopback(parent) {
                warn!("Failed to cleanup loopback: {}", e);
            }
        }
    }
    
    // Remove bootloader
    if let (Some(ref esp_folder), Some(ref bootloader)) = (&state.esp_folder, &state.bootloader_result) {
        if let Err(e) = crate::bootloader::remove_bootloader(esp_folder, &bootloader.boot_entry_id) {
            warn!("Failed to cleanup bootloader: {}", e);
        }
    }
    
    Ok(())
}

/// Trigger system reboot
pub fn reboot() -> Result<()> {
    info!("Initiating system reboot");
    
    #[cfg(windows)]
    {
        std::process::Command::new("shutdown")
            .args(["/r", "/t", "0"])
            .spawn()?;
    }
    
    #[cfg(not(windows))]
    {
        std::process::Command::new("reboot")
            .spawn()?;
    }
    
    Ok(())
}
