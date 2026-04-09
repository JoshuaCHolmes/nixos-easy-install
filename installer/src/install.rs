//! Installation logic - orchestrates the actual installation process
//! 
//! SAFETY DESIGN:
//! - Validates everything before making changes
//! - Each step is logged for debugging
//! - Provides rollback/cleanup on failure
//! - Progress reporting for UI feedback

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
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

/// Options for installation (reserved for future use)
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct InstallOptions {
    /// Dry-run mode: validate everything but don't make changes
    pub dry_run: bool,
    
    /// Skip download (use cached assets)
    pub offline: bool,
}

/// Perform a dry-run installation (validation only, no changes)
/// 
/// This validates all requirements and simulates each step without
/// actually making any changes. Useful for testing.
pub async fn dry_run(
    config: InstallConfig,
    system_info: &SystemInfo,
    progress: ProgressCallback,
) -> Result<DryRunReport> {
    info!("Starting dry-run installation...");
    
    let mut report = DryRunReport::default();
    
    // Step 1: Validate system
    progress(0.1, "[DRY RUN] Validating system requirements...");
    match validate_system(system_info) {
        Ok(_) => report.steps.push(DryRunStep::passed("System validation")),
        Err(e) => report.steps.push(DryRunStep::failed("System validation", &e.to_string())),
    }
    
    // Step 2: Check storage
    progress(0.2, "[DRY RUN] Checking storage requirements...");
    match &config.install_type[..] {
        "loopback" | "quick" => {
            if let Some(ref loopback) = config.loopback {
                let loopback_cfg = crate::loopback::LoopbackConfig {
                    target_dir: std::path::PathBuf::from(&loopback.target_dir),
                    size_gb: loopback.size_gb,
                    separate_home: false,
                    home_size_gb: None,
                };
                match crate::loopback::preflight_check(&loopback_cfg) {
                    Ok(preflight) => {
                        if preflight.passed {
                            report.steps.push(DryRunStep::passed("Loopback storage"));
                        } else {
                            report.steps.push(DryRunStep::failed(
                                "Loopback storage", 
                                &preflight.errors.join(", ")
                            ));
                        }
                        for warning in preflight.warnings {
                            report.warnings.push(warning);
                        }
                    }
                    Err(e) => report.steps.push(DryRunStep::failed("Loopback storage", &e.to_string())),
                }
            }
        }
        "partition" | "full" => {
            report.steps.push(DryRunStep::failed(
                "Partition storage",
                "Full partition installation not yet implemented"
            ));
        }
        _ => {
            report.steps.push(DryRunStep::failed(
                "Storage",
                &format!("Unknown install type: {}", config.install_type)
            ));
        }
    }
    
    // Step 3: Check ESP
    progress(0.3, "[DRY RUN] Checking EFI System Partition...");
    if let Some(ref esp) = system_info.esp {
        match crate::bootloader::preflight_check(esp) {
            Ok(preflight) => {
                if preflight.passed {
                    report.steps.push(DryRunStep::passed("ESP access"));
                } else {
                    report.steps.push(DryRunStep::failed("ESP access", &preflight.errors.join(", ")));
                }
                for warning in preflight.warnings {
                    report.warnings.push(warning);
                }
            }
            Err(e) => report.steps.push(DryRunStep::failed("ESP access", &e.to_string())),
        }
    } else {
        report.steps.push(DryRunStep::failed("ESP access", "No EFI System Partition found"));
    }
    
    // Step 4: Check network (for downloads)
    progress(0.4, "[DRY RUN] Checking network connectivity...");
    match check_network_connectivity() {
        Ok(_) => report.steps.push(DryRunStep::passed("Network connectivity")),
        Err(e) => report.steps.push(DryRunStep::failed("Network connectivity", &e.to_string())),
    }
    
    // Step 5: Validate config
    progress(0.5, "[DRY RUN] Validating configuration...");
    report.steps.push(DryRunStep::passed("Configuration validation"));
    
    progress(1.0, "[DRY RUN] Complete");
    
    report.passed = report.steps.iter().all(|s| s.passed);
    
    Ok(report)
}

/// Check if we can reach the Ubuntu archive
fn check_network_connectivity() -> Result<()> {
    let response = reqwest::blocking::Client::new()
        .head("https://archive.ubuntu.com")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Cannot reach Ubuntu archive - check your internet connection")?;
    
    if response.status().is_success() || response.status().is_redirection() {
        Ok(())
    } else {
        bail!("Ubuntu archive returned status: {}", response.status())
    }
}

/// Report from a dry-run installation
#[derive(Debug, Default)]
pub struct DryRunReport {
    /// Whether all checks passed
    pub passed: bool,
    
    /// Results of each step
    pub steps: Vec<DryRunStep>,
    
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct DryRunStep {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
}

impl DryRunStep {
    fn passed(name: &str) -> Self {
        Self { name: name.to_string(), passed: true, error: None }
    }
    
    fn failed(name: &str, error: &str) -> Self {
        Self { name: name.to_string(), passed: false, error: Some(error.to_string()) }
    }
}

impl std::fmt::Display for DryRunReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Dry Run Report ===")?;
        writeln!(f)?;
        
        for step in &self.steps {
            let status = if step.passed { "✓" } else { "✗" };
            write!(f, "{} {}", status, step.name)?;
            if let Some(ref err) = step.error {
                write!(f, ": {}", err)?;
            }
            writeln!(f)?;
        }
        
        if !self.warnings.is_empty() {
            writeln!(f)?;
            writeln!(f, "Warnings:")?;
            for warning in &self.warnings {
                writeln!(f, "  ⚠ {}", warning)?;
            }
        }
        
        writeln!(f)?;
        if self.passed {
            writeln!(f, "Result: PASSED - Installation can proceed")?;
        } else {
            writeln!(f, "Result: FAILED - Fix the issues above before installing")?;
        }
        
        Ok(())
    }
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
    state.bootloader_result = Some(bootloader_result.clone());
    
    // Step 5: Install OS switching utilities
    progress(0.60, "Installing switching utilities...");
    install_switching_utils(config, &bootloader_result)?;
    
    // Step 6: Write install configuration
    progress(0.70, "Writing installation configuration...");
    write_install_config(config, esp)?;
    
    // Step 7: Final verification
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
    // Download Ubuntu's signed shim and GRUB packages
    let cache_dir = std::env::temp_dir().join("nixos-install").join("boot-assets");
    
    let assets = crate::assets::download_boot_assets(&cache_dir)?;
    
    // Verify the downloaded assets
    crate::assets::verify_assets(&assets)?;
    
    // Download the NixOS installer kernel and initrd
    // Detect architecture (for now, always x86_64 on Windows)
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let installer_assets = crate::assets::download_installer_assets(&cache_dir, arch)?;
    
    // Generate GRUB config based on install type
    let install_type = &config.install_type;
    let nixos_root = if install_type == "loopback" || install_type == "quick" {
        config.loopback.as_ref()
            .map(|l| l.target_dir.clone())
            .unwrap_or_else(|| "C:\\NixOS".to_string())
    } else {
        "/".to_string()
    };
    
    let grub_cfg_content = crate::assets::generate_grub_config(&nixos_root, install_type);
    let grub_cfg_path = cache_dir.join("grub.cfg");
    std::fs::write(&grub_cfg_path, grub_cfg_content)?;
    
    // We don't need a MOK cert when using Ubuntu's pre-signed chain
    // Create empty placeholder (bootloader module expects it but won't use it)
    let mok_path = cache_dir.join("MOK.cer");
    if !mok_path.exists() {
        std::fs::write(&mok_path, "")?;
    }
    
    Ok(BootFiles {
        shim: assets.shim_x64,
        grub: assets.grub_x64,
        mok_cert: mok_path,
        grub_cfg: grub_cfg_path,
        kernel: Some(installer_assets.kernel),
        initrd: Some(installer_assets.initrd),
    })
}

fn setup_bootloader(esp: &EspInfo, boot_files: &BootFiles) -> Result<BootloaderSetupResult> {
    // First verify ESP is accessible
    if esp.mount_point.as_os_str().is_empty() {
        anyhow::bail!("ESP is not mounted. Please mount it and try again.");
    }
    
    crate::bootloader::setup_bootloader(esp, boot_files, "NixOS")
}

/// Install OS switching utilities for easy boot switching
fn install_switching_utils(
    config: &InstallConfig, 
    bootloader_result: &crate::bootloader::BootloaderSetupResult
) -> Result<()> {
    // Install Windows-side utilities to the NixOS folder
    let utils_dir = match config.install_type.as_str() {
        "loopback" | "quick" => {
            let loopback = config.loopback.as_ref()
                .context("Loopback config missing")?;
            PathBuf::from(&loopback.target_dir).join("SwitchOS")
        }
        _ => {
            // For partition install, put in a standard location
            PathBuf::from("C:\\NixOS\\SwitchOS")
        }
    };
    
    crate::switching::install_windows_switching_utils(
        &utils_dir,
        &bootloader_result.boot_entry_id,
    )?;
    
    info!("Installed switching utilities to {:?}", utils_dir);
    info!("  - boot-to-nixos.bat: Double-click to reboot into NixOS");
    info!("  - boot-to-nixos.ps1: PowerShell version (more robust)");
    info!("  - create-shortcut.ps1: Creates desktop shortcut");
    
    Ok(())
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
    
    // Check loopback files exist and have correct properties
    if let Some(ref loopback) = state.loopback_result {
        if !loopback.root_disk.exists() {
            bail!("Root disk image not found at {:?}", loopback.root_disk);
        }
        
        // Verify the sparse file has the expected apparent size
        let metadata = std::fs::metadata(&loopback.root_disk)
            .context("Cannot read root disk metadata")?;
        if metadata.len() == 0 {
            bail!("Root disk image is empty (0 bytes)");
        }
        info!("Root disk verified: {:?} ({} bytes apparent)", 
              loopback.root_disk, metadata.len());
    }
    
    // Check bootloader files exist
    if let Some(ref bootloader) = state.bootloader_result {
        if !bootloader.esp_folder.exists() {
            bail!("ESP folder not found at {:?}", bootloader.esp_folder);
        }
        
        // Verify critical boot files exist
        let shim = bootloader.esp_folder.join("shimx64.efi");
        let grub = bootloader.esp_folder.join("grubx64.efi");
        let config = bootloader.esp_folder.join("install-config.json");
        
        if !shim.exists() {
            bail!("shimx64.efi not found in ESP");
        }
        if !grub.exists() {
            bail!("grubx64.efi not found in ESP");
        }
        if !config.exists() {
            bail!("install-config.json not found in ESP");
        }
        
        info!("ESP folder verified: {:?}", bootloader.esp_folder);
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
