//! Installation logic - the actual installation process

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::InstallConfig;

/// Progress callback type
pub type ProgressCallback = Box<dyn Fn(f32, &str) + Send>;

/// Perform the installation
pub async fn install(config: InstallConfig, progress: ProgressCallback) -> Result<()> {
    info!("Starting installation...");
    
    progress(0.0, "Preparing installation...");
    
    // Step 1: Validate system requirements
    progress(0.05, "Checking system requirements...");
    validate_system()?;
    
    // Step 2: Prepare disk space
    progress(0.10, "Preparing disk space...");
    match config.install_type.as_str() {
        "loopback" => prepare_loopback(&config).await?,
        "partition" => prepare_partitions(&config).await?,
        _ => anyhow::bail!("Unknown install type: {}", config.install_type),
    }
    
    // Step 3: Download/prepare bootloader
    progress(0.30, "Setting up bootloader...");
    setup_bootloader(&config).await?;
    
    // Step 4: Write configuration
    progress(0.50, "Writing installation configuration...");
    write_install_config(&config)?;
    
    // Step 5: Configure boot entry
    progress(0.70, "Configuring boot entry...");
    add_boot_entry(&config)?;
    
    // Step 6: Verify setup
    progress(0.90, "Verifying installation...");
    verify_setup(&config)?;
    
    progress(1.0, "Ready to reboot!");
    
    Ok(())
}

fn validate_system() -> Result<()> {
    let info = crate::system::detect_system()?;
    
    if !info.is_uefi {
        warn!("System is not UEFI - legacy BIOS support is experimental");
    }
    
    // Check disk space from system info - use total available from largest disk
    let max_disk_space = info.disks.iter()
        .flat_map(|d| d.partitions.iter())
        .filter_map(|p| p.free_space)
        .max()
        .unwrap_or(0);
    
    if max_disk_space < 20 * 1024 * 1024 * 1024 {
        anyhow::bail!("Insufficient disk space. At least 20GB required.");
    }
    
    Ok(())
}

async fn prepare_loopback(config: &InstallConfig) -> Result<()> {
    let loopback = config.loopback.as_ref()
        .context("Loopback config missing")?;
    
    let target_dir = Path::new(&loopback.target_dir);
    
    // Create target directory
    info!("Creating NixOS directory at {:?}", target_dir);
    std::fs::create_dir_all(target_dir)?;
    
    // Create root.disk file
    let root_disk = target_dir.join("root.disk");
    if !root_disk.exists() {
        info!("Creating root.disk ({} GB)", loopback.size_gb);
        // TODO: Actually create the file with proper size
        // This would use sparse file creation on Windows
    }
    
    Ok(())
}

async fn prepare_partitions(config: &InstallConfig) -> Result<()> {
    let partition = config.partition.as_ref()
        .context("Partition config missing")?;
    
    info!("Preparing partitions: root={}, boot={}", partition.root, partition.boot);
    
    // TODO: Use diskpart to shrink Windows partition and create new partitions
    // This is the most dangerous part - needs careful implementation
    
    warn!("Partition preparation not yet implemented");
    
    Ok(())
}

async fn setup_bootloader(config: &InstallConfig) -> Result<()> {
    info!("Setting up EFI bootloader");
    
    // TODO: 
    // 1. Find EFI System Partition
    // 2. Copy shimx64.efi, grubx64.efi, etc. to ESP/EFI/nixos/
    // 3. Copy MOK key for Secure Boot
    // 4. Copy NixOS installer kernel + initrd
    
    Ok(())
}

fn write_install_config(config: &InstallConfig) -> Result<()> {
    // Write config to EFI partition so the installer initrd can read it
    let config_json = config.to_json()?;
    
    // TODO: Find ESP and write to ESP/nixos-install/install-config.json
    info!("Install config:\n{}", config_json);
    
    Ok(())
}

fn add_boot_entry(config: &InstallConfig) -> Result<()> {
    info!("Adding UEFI boot entry for NixOS installer");
    
    // TODO: Use bcdedit or direct EFI variable manipulation to add boot entry
    // Set it as the default for next boot
    
    Ok(())
}

fn verify_setup(config: &InstallConfig) -> Result<()> {
    info!("Verifying installation setup");
    
    // TODO: Check that all files are in place, boot entry exists, etc.
    
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
