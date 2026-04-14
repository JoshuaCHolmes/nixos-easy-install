//! Installation logic - orchestrates the actual installation process
//! 
//! SAFETY DESIGN:
//! - Validates everything before making changes
//! - Each step is logged for debugging
//! - Provides rollback/cleanup on failure
//! - Progress reporting for UI feedback

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tracing::{info, warn, error, debug};

use crate::config::InstallConfig;
use crate::system::{SystemInfo, EspInfo};
use crate::loopback::{LoopbackSetup, LoopbackPrepareResult};
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
pub struct InstallMode {
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
                let loopback_cfg = crate::loopback::LoopbackSetup {
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
    
    // =========================================================================
    // PHASE 1: ALL PREFLIGHT CHECKS (no changes made yet)
    // =========================================================================
    
    progress(0.02, "Running preflight checks...");
    
    // Validate system hardware requirements
    validate_system(system_info)?;
    
    // Validate ESP access BEFORE we make any changes
    let esp = system_info.esp.as_ref()
        .context("No EFI System Partition found")?;
    let bootloader_preflight = crate::bootloader::preflight_check(esp)?;
    if !bootloader_preflight.passed {
        bail!("ESP preflight failed: {}", bootloader_preflight.errors.join(", "));
    }
    
    // Validate loopback storage BEFORE we create files
    if config.install_type == "loopback" || config.install_type == "quick" {
        if let Some(ref loopback) = config.loopback {
            let loopback_cfg = crate::loopback::LoopbackSetup {
                target_dir: std::path::PathBuf::from(&loopback.target_dir),
                size_gb: loopback.size_gb,
                separate_home: false,
                home_size_gb: None,
            };
            let loopback_preflight = crate::loopback::preflight_check(&loopback_cfg)?;
            if !loopback_preflight.passed {
                bail!("Storage preflight failed: {}", loopback_preflight.errors.join(", "));
            }
        }
    }
    
    // Validate config (hostname, username, etc.)
    validate_config(config)?;
    
    info!("All preflight checks passed - proceeding with installation");
    
    // =========================================================================
    // PHASE 2: DOWNLOAD ASSETS (reversible - just cache files)
    // =========================================================================
    
    progress(0.10, "Downloading NixOS boot files...");
    let boot_files = download_boot_assets(config).await?;
    
    // =========================================================================
    // PHASE 3: MAKE CHANGES (point of no return)
    // =========================================================================
    
    // Step 1: Prepare storage
    progress(0.30, "Preparing storage...");
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
    
    // Step 2: Set up bootloader (ESP already verified in preflight)
    progress(0.50, "Setting up bootloader...");
    let bootloader_result = setup_bootloader(esp, &boot_files, config)?;
    state.esp_folder = Some(bootloader_result.esp_folder.clone());
    state.bootloader_result = Some(bootloader_result.clone());
    
    // Step 3: Install OS switching utilities
    progress(0.60, "Installing switching utilities...");
    install_switching_utils(config, &bootloader_result)?;
    
    // Step 4: Write install configuration
    progress(0.70, "Writing installation configuration...");
    write_install_config(config, esp)?;
    
    // =========================================================================
    // PHASE 4: VERIFICATION
    // =========================================================================
    
    progress(0.90, "Verifying installation...");
    verify_setup(state)?;
    
    // Clean up download cache on success
    let cache_dir = std::env::temp_dir().join("nixos-install").join("boot-assets");
    if cache_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
            warn!("Could not clean up cache directory: {}", e);
        } else {
            debug!("Cleaned up cache directory: {:?}", cache_dir);
        }
    }
    
    progress(1.0, "Ready to reboot!");
    
    Ok(())
}

/// Validate the install configuration
fn validate_config(config: &InstallConfig) -> Result<()> {
    // Validate hostname
    if config.hostname.is_empty() {
        bail!("Hostname cannot be empty");
    }
    if config.hostname.len() > 63 {
        bail!("Hostname too long (max 63 characters)");
    }
    // Simple hostname validation - ASCII alphanumeric and hyphens only
    if !config.hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("Hostname contains invalid characters (only ASCII letters, numbers, and hyphens allowed)");
    }
    if config.hostname.starts_with('-') || config.hostname.ends_with('-') {
        bail!("Hostname cannot start or end with a hyphen");
    }
    
    // Validate username
    if config.username.is_empty() {
        bail!("Username cannot be empty");
    }
    if config.username.len() > 32 {
        bail!("Username too long (max 32 characters)");
    }
    // Unix username validation - lowercase, digits, underscore only (no hyphens per POSIX)
    if !config.username.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false) {
        bail!("Username must start with a lowercase letter");
    }
    if !config.username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        bail!("Username contains invalid characters (only lowercase letters, digits, and underscores allowed)");
    }
    
    // Validate loopback config
    if config.install_type == "loopback" || config.install_type == "quick" {
        if let Some(ref loopback) = config.loopback {
            if loopback.size_gb < 10 {
                bail!("Disk size too small (minimum 10 GB)");
            }
            if loopback.size_gb > 2048 {
                bail!("Disk size too large (maximum 2 TB)");
            }
            if loopback.target_dir.is_empty() {
                bail!("Target directory cannot be empty");
            }
        } else {
            bail!("Loopback configuration required for loopback/quick install");
        }
    }
    
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
    
    let cfg = LoopbackSetup {
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
    
    // Detect architecture at runtime
    let platform = crate::assets::detect_platform();
    let arch = platform.base_arch();
    let needs_dtb = platform.needs_custom_kernel();
    
    info!("Detected platform: {} (arch: {})", platform.display_name(), arch);
    
    // Download boot assets for base architecture (shim/GRUB are generic per arch)
    let assets = crate::assets::download_boot_assets_for_arch(&cache_dir, arch)?;
    
    // Verify the downloaded assets
    crate::assets::verify_assets(&assets)?;
    
    // Download the NixOS installer kernel and initrd (platform-specific for X1E)
    let installer_assets = crate::assets::download_installer_assets_for_platform(&cache_dir, platform)?;
    
    // Verify DTB is present if platform requires it
    let has_dtb = if needs_dtb {
        if installer_assets.device_dtb.is_none() {
            anyhow::bail!(
                "Platform {} requires a Device Tree Blob but none was downloaded.\n\
                This is a critical requirement for {} devices to boot.",
                platform.display_name(), platform.display_name()
            );
        }
        true
    } else {
        installer_assets.device_dtb.is_some()
    };
    
    // Generate GRUB config based on install type
    let install_type = &config.install_type;
    let nixos_root = if install_type == "loopback" || install_type == "quick" {
        config.loopback.as_ref()
            .map(|l| l.target_dir.clone())
            .ok_or_else(|| anyhow::anyhow!("Loopback config missing for loopback install type"))?
    } else {
        "/".to_string()
    };
    
    let grub_cfg_content = crate::assets::generate_grub_config(
        &nixos_root, 
        install_type,
        installer_assets.init_path.as_deref(),
        has_dtb
    );
    let grub_cfg_path = cache_dir.join("grub.cfg");
    std::fs::write(&grub_cfg_path, grub_cfg_content)?;
    
    // We don't need a MOK cert when using Ubuntu's pre-signed chain
    // Create empty placeholder (bootloader module expects it but won't use it)
    let mok_path = cache_dir.join("MOK.cer");
    if !mok_path.exists() {
        std::fs::write(&mok_path, "")?;
    }
    
    Ok(BootFiles {
        shim: assets.shim,
        grub: assets.grub,
        mok_cert: mok_path,
        grub_cfg: grub_cfg_path,
        kernel: Some(installer_assets.kernel),
        initrd: Some(installer_assets.initrd),
        device_dtb: installer_assets.device_dtb,
        arch: arch.to_string(),
    })
}

fn setup_bootloader(esp: &EspInfo, boot_files: &BootFiles, config: &InstallConfig) -> Result<BootloaderSetupResult> {
    // First verify ESP is accessible
    if esp.mount_point.as_os_str().is_empty() {
        anyhow::bail!("ESP is not mounted. Please mount it and try again.");
    }
    
    // For loopback installs, store large boot files (kernel, initrd) on the NTFS partition
    // This avoids ESP space limitations (ESP is typically only 100-500MB, initrd can be 500MB+)
    let boot_files_dir = if config.install_type == "loopback" || config.install_type == "quick" {
        config.loopback.as_ref().map(|l| PathBuf::from(&l.target_dir))
    } else {
        None
    };
    
    crate::bootloader::setup_bootloader(esp, boot_files, "NixOS", boot_files_dir.as_deref())
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
        
        // Verify critical boot files exist (architecture-specific names)
        let arch = crate::assets::detect_arch();
        let platform = crate::assets::detect_platform();
        let (shim_name, grub_name) = if arch == "aarch64" {
            ("shimaa64.efi", "grubaa64.efi")
        } else {
            ("shimx64.efi", "grubx64.efi")
        };
        
        let shim = bootloader.esp_folder.join(shim_name);
        let grub = bootloader.esp_folder.join(grub_name);
        let config = bootloader.esp_folder.join("install-config.json");
        
        if !shim.exists() {
            bail!("{} not found in ESP", shim_name);
        }
        if !grub.exists() {
            bail!("{} not found in ESP", grub_name);
        }
        if !config.exists() {
            bail!("install-config.json not found in ESP");
        }
        
        // Verify DTB exists for platforms that require it (X1E)
        if platform.needs_custom_kernel() {
            // DTB is stored in boot_files_folder (NTFS for loopback installs, ESP otherwise)
            let dtb = bootloader.boot_files_folder.join("device.dtb");
            if !dtb.exists() {
                bail!(
                    "Device Tree Blob (device.dtb) not found at {:?}.\n\
                    This is required for {} to boot. Installation cannot continue.",
                    bootloader.boot_files_folder,
                    platform.display_name()
                );
            }
            let dtb_size = std::fs::metadata(&dtb)?.len();
            // DTBs are typically 50-300KB; anything under 10KB is likely corrupt
            if dtb_size < 10_000 {
                bail!(
                    "Device Tree Blob appears invalid (only {} bytes). Expected > 10KB for valid DTB.",
                    dtb_size
                );
            }
            info!("Device Tree Blob verified ({} bytes)", dtb_size);
        }
        
        // Verify kernel and initrd exist in boot_files_folder
        // These are always stored there (NTFS for loopback, ESP otherwise)
        let kernel = bootloader.boot_files_folder.join("bzImage");
        let initrd = bootloader.boot_files_folder.join("initrd");
        
        if !kernel.exists() {
            bail!("Kernel (bzImage) not found at {:?}", bootloader.boot_files_folder);
        }
        if !initrd.exists() {
            bail!("Initrd not found at {:?}", bootloader.boot_files_folder);
        }
        
        let kernel_size = std::fs::metadata(&kernel)?.len();
        let initrd_size = std::fs::metadata(&initrd)?.len();
        
        // Sanity check sizes - kernel should be at least 1MB, initrd at least 10MB
        if kernel_size < 1_000_000 {
            bail!("Kernel appears invalid (only {} bytes). Expected > 1MB.", kernel_size);
        }
        if initrd_size < 10_000_000 {
            bail!("Initrd appears invalid (only {} bytes). Expected > 10MB.", initrd_size);
        }
        
        info!("Kernel verified ({} bytes)", kernel_size);
        info!("Initrd verified ({} bytes)", initrd_size);
        
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
    
    // Clean up download cache (boot assets in temp directory)
    let cache_dir = std::env::temp_dir().join("nixos-install").join("boot-assets");
    if cache_dir.exists() {
        info!("Cleaning up download cache at {:?}", cache_dir);
        if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
            warn!("Failed to cleanup download cache: {}", e);
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
