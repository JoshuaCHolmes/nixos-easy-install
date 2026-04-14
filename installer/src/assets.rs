//! Boot asset downloading and extraction
//! 
//! Downloads Ubuntu's signed shim and GRUB packages, extracts the EFI binaries.
//! These are Microsoft-signed (shim) and Canonical-signed (GRUB), so they work
//! with Secure Boot out of the box on most systems.

// Some constants are reserved for fallback/verification scenarios
#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{Read, Write, Cursor};
use std::time::Duration;
use tracing::{info, debug, warn};

// ============================================================================
// ESP Space Requirements
// ============================================================================
// These are approximate sizes of assets we place on ESP.
// Based on actual build outputs. Update if initrd/kernel sizes change significantly.

/// Shim + GRUB + configs: ~5MB
pub const ESP_SIZE_BOOTLOADER: u64 = 5 * 1024 * 1024;

/// Kernel (bzImage): x86_64 ~45MB, aarch64 ~62MB
/// We use architecture-specific sizes for accuracy
pub const ESP_SIZE_KERNEL_X64: u64 = 48 * 1024 * 1024;
pub const ESP_SIZE_KERNEL_AA64: u64 = 65 * 1024 * 1024;

/// Initrd: ~13MB on both architectures
pub const ESP_SIZE_INITRD: u64 = 15 * 1024 * 1024;

/// Safety margin for configs, GRUB modules, filesystem overhead
pub const ESP_SIZE_MARGIN: u64 = 2 * 1024 * 1024;

/// Calculate total ESP space required for current architecture
pub fn required_esp_space() -> u64 {
    let kernel_size = if detect_arch() == "aarch64" {
        ESP_SIZE_KERNEL_AA64
    } else {
        ESP_SIZE_KERNEL_X64
    };
    ESP_SIZE_BOOTLOADER + kernel_size + ESP_SIZE_INITRD + ESP_SIZE_MARGIN
}

/// Get human-readable ESP space requirement in MB
pub fn required_esp_space_mb() -> u64 {
    required_esp_space() / (1024 * 1024)
}

// ============================================================================
// Boot Asset URLs and Checksums
// ============================================================================

/// URLs for Ubuntu's signed boot packages (Noble 24.04 LTS)
/// x86_64 (amd64) packages from archive.ubuntu.com
const SHIM_SIGNED_URL_X64: &str = "https://archive.ubuntu.com/ubuntu/pool/main/s/shim-signed/shim-signed_1.59+15.8-0ubuntu2_amd64.deb";
const GRUB_SIGNED_URL_X64: &str = "https://archive.ubuntu.com/ubuntu/pool/main/g/grub2-signed/grub-efi-amd64-signed_1.215+2.14-2ubuntu1_amd64.deb";

/// ARM64 (aarch64) packages from ports.ubuntu.com
const SHIM_SIGNED_URL_AA64: &str = "https://ports.ubuntu.com/pool/main/s/shim-signed/shim-signed_1.59+15.8-0ubuntu2_arm64.deb";
const GRUB_SIGNED_URL_AA64: &str = "https://ports.ubuntu.com/pool/main/g/grub2-signed/grub-efi-arm64-signed_1.215+2.14-2ubuntu1_arm64.deb";

/// SHA256 checksums for integrity verification
/// These are the checksums of the .deb packages from Ubuntu's official repos
const SHIM_SIGNED_SHA256_X64: &str = "f8ed71ce2d91a304b6d5eb84997f846f331b554578bc02dbfe78e13ad8ac81a9";
const GRUB_SIGNED_SHA256_X64: &str = "603fe7db065634780d9576bab48fce8143a0451697c5be75a6cdb1f6a5e39188";
const SHIM_SIGNED_SHA256_AA64: &str = "48f6301b173cf306cb2ae52aee0b290ded10d01c782fec4b29d73cd5621a5cc4";
const GRUB_SIGNED_SHA256_AA64: &str = "67666b3c1b97865addb26b4f8fa4b8ca19a62c49466b3a72902f301578dd7bdd";

/// Expected files after extraction
#[derive(Debug)]
pub struct BootAssets {
    /// Microsoft-signed shim (first-stage loader)
    pub shim: PathBuf,
    
    /// MokManager for key enrollment (if needed)
    pub mok_manager: PathBuf,
    
    /// Fallback bootloader
    pub fallback: PathBuf,
    
    /// Canonical-signed GRUB
    pub grub: PathBuf,
    
    /// Directory containing all assets
    pub asset_dir: PathBuf,
    
    /// Architecture (x86_64 or aarch64)
    pub arch: String,
}

/// Detect the system architecture at runtime
pub fn detect_arch() -> &'static str {
    // On Windows ARM64 running x86_64 emulation:
    // - PROCESSOR_ARCHITECTURE = "AMD64" (emulated)
    // - PROCESSOR_ARCHITEW6432 = not set (only set for WoW64 32-bit on 64-bit)
    // 
    // We need to check the *native* architecture, not the emulated one.
    // Use a Windows API call or check alternative env vars.
    
    // First, check if we're running under ARM64 emulation
    // The presence of certain ARM64-specific paths or registry can indicate this
    if let Ok(arch) = std::env::var("PROCESSOR_ARCHITECTURE") {
        tracing::debug!("PROCESSOR_ARCHITECTURE = {}", arch);
        
        if arch == "ARM64" {
            tracing::info!("Detected native ARM64");
            return "aarch64";
        }
    }
    
    // Check PROCESSOR_ARCHITEW6432 (set when 32-bit process runs on 64-bit Windows)
    if let Ok(arch) = std::env::var("PROCESSOR_ARCHITEW6432") {
        tracing::debug!("PROCESSOR_ARCHITEW6432 = {}", arch);
        if arch == "ARM64" {
            tracing::info!("Detected ARM64 via PROCESSOR_ARCHITEW6432");
            return "aarch64";
        }
    }
    
    // Use WMI or registry to detect actual hardware architecture
    // Check for ARM64 indicator in ProgramFiles path
    if let Ok(pf) = std::env::var("ProgramFiles(Arm)") {
        if !pf.is_empty() {
            tracing::info!("Detected ARM64 via ProgramFiles(Arm) = {}", pf);
            return "aarch64";
        }
    }
    
    // Check registry for ARM64 - the existence of ARM64 emulation settings indicates ARM64 hardware
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // Query system info via wmic
        if let Ok(output) = Command::new("wmic")
            .args(["os", "get", "osarchitecture"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::debug!("WMIC OSArchitecture: {}", stdout);
            if stdout.contains("ARM") {
                tracing::info!("Detected ARM64 via WMIC");
                return "aarch64";
            }
        }
    }
    
    tracing::info!("Defaulting to x86_64 architecture");
    "x86_64"
}

/// Supported device platforms with specialized hardware support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwarePlatform {
    /// Standard x86_64 (Intel/AMD)
    X86_64,
    /// Standard ARM64 (generic)  
    Aarch64,
    /// Snapdragon X Elite (X1E80100) - requires custom kernel
    SnapdragonX1E,
}

impl HardwarePlatform {
    /// Get the architecture string for downloads
    pub fn arch_string(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::SnapdragonX1E => "aarch64-x1e",
        }
    }
    
    /// Get the base architecture (for shim/GRUB downloads)
    pub fn base_arch(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 | Self::SnapdragonX1E => "aarch64",
        }
    }
    
    /// Whether this platform requires special kernel support
    pub fn needs_custom_kernel(&self) -> bool {
        matches!(self, Self::SnapdragonX1E)
    }
    
    /// Get a human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::X86_64 => "Intel/AMD (x86_64)",
            Self::Aarch64 => "ARM64 (generic)",
            Self::SnapdragonX1E => "Snapdragon X Elite",
        }
    }
}

/// Detect if the system is a Snapdragon X Elite device
/// 
/// Snapdragon X Elite (X1E80100) devices need special kernel support from
/// x1e-nixos-config due to unique hardware requirements (device trees, 
/// kernel parameters, firmware blobs).
pub fn detect_snapdragon_x1e() -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        return false;
    }
    
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        
        // First check if we're even on ARM64
        if detect_arch() != "aarch64" {
            return false;
        }
        
        // Primary detection: Check for Qualcomm/Snapdragon processor via WMIC
        if let Ok(output) = Command::new("wmic")
            .args(["cpu", "get", "name"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::debug!("WMIC CPU Name: {}", stdout);
            
            // Snapdragon X Elite/Plus variants (X1E80100, X1E78100, X1P64100, etc.)
            if stdout.contains("Snapdragon") && (
                stdout.contains("X Elite") || 
                stdout.contains("X1E") ||
                stdout.contains("X Plus") ||
                stdout.contains("X1P")
            ) {
                tracing::info!("Detected Snapdragon X Elite via CPU name: {}", stdout.trim());
                return true;
            }
            
            // Also check for Qualcomm Oryon cores (the CPU architecture used in X Elite)
            if stdout.contains("Qualcomm") && stdout.contains("Oryon") {
                tracing::info!("Detected Qualcomm Oryon CPU (Snapdragon X series)");
                return true;
            }
        }
        
        // Fallback: Check via PowerShell (sometimes more reliable for ARM)
        if let Ok(output) = Command::new("powershell")
            .args(["-NoProfile", "-Command", 
                   "(Get-WmiObject -Class Win32_Processor).Name"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::debug!("PowerShell CPU Name: {}", stdout);
            
            if stdout.contains("Snapdragon") && (
                stdout.contains("X Elite") || 
                stdout.contains("X Plus") ||
                stdout.contains("X1E") ||
                stdout.contains("X1P")
            ) {
                tracing::info!("Detected Snapdragon X Elite via PowerShell");
                return true;
            }
        }
        
        // Note: We don't use model-based detection because:
        // 1. CPU name detection is more reliable
        // 2. Model names like "Surface Pro" are ambiguous (both ARM and x86 variants exist)
        // 3. We already verified ARM64 architecture above
        
        // If we're on ARM64 but can't detect Snapdragon specifically, 
        // log it for debugging but don't assume X1E
        tracing::debug!("ARM64 detected but Snapdragon X Elite not confirmed via CPU name");
        false
    }
}

/// Get the full hardware platform (with Snapdragon detection)
pub fn detect_platform() -> HardwarePlatform {
    let arch = detect_arch();
    
    if arch == "aarch64" {
        if detect_snapdragon_x1e() {
            return HardwarePlatform::SnapdragonX1E;
        }
        return HardwarePlatform::Aarch64;
    }
    
    HardwarePlatform::X86_64
}

/// Download and extract boot assets for the detected architecture
/// 
/// Returns paths to all required EFI binaries for Secure Boot
pub fn download_boot_assets(cache_dir: &Path) -> Result<BootAssets> {
    let arch = detect_arch();
    download_boot_assets_for_arch(cache_dir, arch)
}

/// Download and extract boot assets for a specific architecture
pub fn download_boot_assets_for_arch(cache_dir: &Path, arch: &str) -> Result<BootAssets> {
    info!("Downloading {} boot assets to {:?}", arch, cache_dir);
    
    fs::create_dir_all(cache_dir)?;
    
    // Normalize architecture - X1E uses same shim/GRUB as generic aarch64
    let base_arch = if arch.starts_with("aarch64") { "aarch64" } else { "x86_64" };
    
    // Determine filenames based on architecture
    let (shim_name, grub_name, mm_name, fb_name) = if base_arch == "aarch64" {
        ("shimaa64.efi", "grubaa64.efi", "mmaa64.efi", "fbaa64.efi")
    } else {
        ("shimx64.efi", "grubx64.efi", "mmx64.efi", "fbx64.efi")
    };
    
    let assets = BootAssets {
        shim: cache_dir.join(shim_name),
        mok_manager: cache_dir.join(mm_name),
        fallback: cache_dir.join(fb_name),
        grub: cache_dir.join(grub_name),
        asset_dir: cache_dir.to_path_buf(),
        arch: base_arch.to_string(),
    };
    
    // Check cache
    if assets.shim.exists() && assets.grub.exists() {
        info!("Using cached boot assets");
        return Ok(assets);
    }
    
    // Select URLs based on architecture
    let (shim_url, grub_url, grub_signed_name) = if base_arch == "aarch64" {
        (SHIM_SIGNED_URL_AA64, GRUB_SIGNED_URL_AA64, "grubaa64.efi.signed")
    } else {
        (SHIM_SIGNED_URL_X64, GRUB_SIGNED_URL_X64, "grubx64.efi.signed")
    };
    
    // Get checksums for verification
    let (shim_sha256, grub_sha256) = if base_arch == "aarch64" {
        (SHIM_SIGNED_SHA256_AA64, GRUB_SIGNED_SHA256_AA64)
    } else {
        (SHIM_SIGNED_SHA256_X64, GRUB_SIGNED_SHA256_X64)
    };
    
    // Download shim-signed with checksum verification
    info!("Downloading shim-signed package for {}...", base_arch);
    let shim_deb = download_file_with_checksum(shim_url, cache_dir, "shim-signed.deb", Some(shim_sha256))?;
    extract_deb_efi_files(&shim_deb, cache_dir, &[shim_name, mm_name, fb_name])?;
    fs::remove_file(&shim_deb)?;
    
    // Download grub-signed with checksum verification
    info!("Downloading grub-signed package for {}...", base_arch);
    let grub_deb = download_file_with_checksum(grub_url, cache_dir, "grub-signed.deb", Some(grub_sha256))?;
    extract_deb_efi_files(&grub_deb, cache_dir, &[grub_signed_name])?;
    fs::remove_file(&grub_deb)?;
    
    // Rename signed grub to standard name
    let signed_grub = cache_dir.join(grub_signed_name);
    if signed_grub.exists() {
        fs::rename(&signed_grub, &assets.grub)?;
    }
    
    // Verify extraction
    if !assets.shim.exists() {
        bail!("Failed to extract {}", shim_name);
    }
    if !assets.grub.exists() {
        bail!("Failed to extract {}", grub_name);
    }
    
    // Verify EFI executables have valid PE headers
    verify_efi_executable(&assets.shim, shim_name)?;
    verify_efi_executable(&assets.grub, grub_name)?;
    
    info!("Boot assets ready for {}", arch);
    Ok(assets)
}

/// Verify that a file is a valid PE/EFI executable
fn verify_efi_executable(path: &Path, name: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 2];
    file.read_exact(&mut header)?;
    
    // DOS/PE header starts with "MZ"
    if header != [0x4D, 0x5A] {
        bail!(
            "{} is not a valid EFI executable (expected MZ header, got {:02X} {:02X})",
            name, header[0], header[1]
        );
    }
    
    debug!("{} has valid PE header", name);
    Ok(())
}

/// Download a file from URL to destination with optional SHA256 verification
fn download_file(url: &str, dir: &Path, filename: &str) -> Result<PathBuf> {
    download_file_with_checksum(url, dir, filename, None)
}

/// Installer boot assets (kernel + initrd + init path + optional DTB)
#[derive(Debug)]
pub struct InstallerAssets {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    /// Path to the NixOS init (read from init-path file)
    pub init_path: Option<String>,
    /// Hardware platform this was built for
    pub platform: Option<String>,
    /// Device Tree Blob for ARM64 devices (required for X1E)
    pub device_dtb: Option<PathBuf>,
}

/// GitHub release URL for installer boot assets
const INSTALLER_RELEASE_BASE: &str = "https://github.com/JoshuaCHolmes/nixos-easy-install/releases/latest/download";

/// Download the NixOS installer kernel and initrd for the detected platform
/// 
/// Automatically detects Snapdragon X Elite and downloads the appropriate kernel
pub fn download_installer_assets_auto(cache_dir: &Path) -> Result<InstallerAssets> {
    let platform = detect_platform();
    info!("Detected platform: {}", platform.display_name());
    download_installer_assets_for_platform(cache_dir, platform)
}

/// Download the NixOS installer kernel and initrd for a specific platform
pub fn download_installer_assets_for_platform(cache_dir: &Path, platform: HardwarePlatform) -> Result<InstallerAssets> {
    let arch_str = platform.arch_string();
    info!("Downloading installer boot files for {} ({})...", platform.display_name(), arch_str);
    
    fs::create_dir_all(cache_dir)?;
    
    let init_path_file = cache_dir.join("init-path");
    let platform_file = cache_dir.join("platform");
    let dtb_file = cache_dir.join("device.dtb");
    let mut assets = InstallerAssets {
        kernel: cache_dir.join("bzImage"),
        initrd: cache_dir.join("initrd"),
        init_path: None,
        platform: None,
        device_dtb: None,
    };
    
    // Check cache - but verify platform matches AND required files exist
    if assets.kernel.exists() && assets.initrd.exists() && init_path_file.exists() {
        let cached_platform = fs::read_to_string(&platform_file).ok().map(|s| s.trim().to_string());
        
        // For X1E platforms, DTB must also exist in cache
        let dtb_required = platform.needs_custom_kernel();
        let dtb_present = dtb_file.exists();
        
        // If platform matches (or no platform file exists for older cache), use cache
        // But for X1E, also require DTB to be present
        let cache_valid = if cached_platform.as_deref() == Some(arch_str) {
            // Platform matches - for X1E, also need DTB
            !dtb_required || dtb_present
        } else if cached_platform.is_none() && !platform.needs_custom_kernel() {
            // Old cache without platform file, but we're not X1E so it's ok
            true
        } else {
            false
        };
        
        if cache_valid {
            assets.init_path = fs::read_to_string(&init_path_file).ok().map(|s| s.trim().to_string());
            assets.platform = cached_platform;
            // Check for DTB (X1E only)
            if dtb_present {
                assets.device_dtb = Some(dtb_file);
            }
            info!("Using cached installer boot files");
            return Ok(assets);
        } else {
            if dtb_required && !dtb_present {
                info!("Cache missing required DTB for {}, re-downloading...", platform.display_name());
            } else {
                info!("Platform changed from {:?} to {}, re-downloading...", cached_platform, arch_str);
            }
            // Clear stale cache completely
            let _ = fs::remove_file(&assets.kernel);
            let _ = fs::remove_file(&assets.initrd);
            let _ = fs::remove_file(&init_path_file);
            let _ = fs::remove_file(&platform_file);
            let _ = fs::remove_file(&dtb_file);
        }
    }
    
    // Download tarball
    let tarball_name = format!("nixos-installer-{}.tar.gz", arch_str);
    let tarball_url = format!("{}/{}", INSTALLER_RELEASE_BASE, tarball_name);
    
    info!("Downloading {}...", tarball_name);
    let tarball_path = download_file(&tarball_url, cache_dir, &tarball_name)?;
    
    // Try to download and verify checksums (warn if unavailable)
    let checksums_url = format!("{}/SHA256SUMS.txt", INSTALLER_RELEASE_BASE);
    match download_file(&checksums_url, cache_dir, "SHA256SUMS.txt") {
        Ok(checksums_path) => {
            // Verify tarball checksum
            if let Err(e) = verify_file_checksum(&tarball_path, &checksums_path, &tarball_name) {
                warn!("Checksum verification failed: {}. Proceeding anyway.", e);
            } else {
                info!("Checksum verified for {}", tarball_name);
            }
            let _ = fs::remove_file(&checksums_path);
        }
        Err(e) => {
            warn!("Could not download SHA256SUMS.txt: {}. Skipping verification.", e);
        }
    }
    
    // Extract tarball
    info!("Extracting installer boot files...");
    extract_tarball(&tarball_path, cache_dir, arch_str)?;
    
    // Clean up tarball
    let _ = fs::remove_file(&tarball_path);
    
    // Verify extraction
    if !assets.kernel.exists() {
        bail!("Failed to extract kernel (bzImage)");
    }
    if !assets.initrd.exists() {
        bail!("Failed to extract initrd");
    }
    
    // Read init path if available
    if init_path_file.exists() {
        let init_path = fs::read_to_string(&init_path_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if init_path.is_none() {
            warn!("init-path file exists but is empty - boot will likely fail!");
        }
        assets.init_path = init_path;
        info!("Init path: {:?}", assets.init_path);
    } else {
        warn!("No init-path file found - kernel may fail to boot without init= parameter");
    }
    
    // Check for DTB (X1E platform only - REQUIRED for boot)
    if dtb_file.exists() {
        assets.device_dtb = Some(dtb_file.clone());
        info!("Device Tree Blob found for hardware initialization");
    } else if platform.needs_custom_kernel() {
        bail!(
            "Device Tree Blob (DTB) required for {} but not found.\n\
            The DTB is essential for hardware initialization on this platform.\n\
            Please ensure you're using the correct installer release for your hardware.",
            platform.display_name()
        );
    }
    
    // Store platform for cache validation
    assets.platform = Some(arch_str.to_string());
    fs::write(&platform_file, arch_str)?;
    
    info!("Installer boot files ready for {}", platform.display_name());
    Ok(assets)
}

/// Download the NixOS installer kernel and initrd (legacy - uses base arch only)
/// 
/// These are pre-built from the initrd/default.nix and uploaded to GitHub releases
pub fn download_installer_assets(cache_dir: &Path, arch: &str) -> Result<InstallerAssets> {
    info!("Downloading installer boot files for {}...", arch);
    
    fs::create_dir_all(cache_dir)?;
    
    let init_path_file = cache_dir.join("init-path");
    let mut assets = InstallerAssets {
        kernel: cache_dir.join("bzImage"),
        initrd: cache_dir.join("initrd"),
        init_path: None,
        platform: None,
        device_dtb: None,
    };
    
    // Check cache
    if assets.kernel.exists() && assets.initrd.exists() && init_path_file.exists() {
        // Read cached init path
        assets.init_path = fs::read_to_string(&init_path_file).ok().map(|s| s.trim().to_string());
        info!("Using cached installer boot files");
        return Ok(assets);
    }
    
    // Download tarball
    let tarball_name = format!("nixos-installer-{}.tar.gz", arch);
    let tarball_url = format!("{}/{}", INSTALLER_RELEASE_BASE, tarball_name);
    
    info!("Downloading {}...", tarball_name);
    let tarball_path = download_file(&tarball_url, cache_dir, &tarball_name)?;
    
    // Try to download and verify checksums (warn if unavailable)
    let checksums_url = format!("{}/SHA256SUMS.txt", INSTALLER_RELEASE_BASE);
    match download_file(&checksums_url, cache_dir, "SHA256SUMS.txt") {
        Ok(checksums_path) => {
            // Verify tarball checksum
            if let Err(e) = verify_file_checksum(&tarball_path, &checksums_path, &tarball_name) {
                warn!("Checksum verification failed: {}. Proceeding anyway.", e);
            } else {
                info!("Checksum verified for {}", tarball_name);
            }
            let _ = fs::remove_file(&checksums_path);
        }
        Err(e) => {
            warn!("Could not download SHA256SUMS.txt: {}. Skipping verification.", e);
        }
    }
    
    // Extract tarball
    info!("Extracting installer boot files...");
    extract_tarball(&tarball_path, cache_dir, arch)?;
    
    // Clean up tarball
    let _ = fs::remove_file(&tarball_path);
    
    // Verify extraction
    if !assets.kernel.exists() {
        bail!("Failed to extract kernel (bzImage)");
    }
    if !assets.initrd.exists() {
        bail!("Failed to extract initrd");
    }
    
    // Read init path if available
    let init_path_file = cache_dir.join("init-path");
    if init_path_file.exists() {
        assets.init_path = fs::read_to_string(&init_path_file).ok().map(|s| s.trim().to_string());
        info!("Init path: {:?}", assets.init_path);
    } else {
        warn!("No init-path file found - kernel may fail to boot without init= parameter");
    }
    
    info!("Installer boot files ready");
    Ok(assets)
}

/// Verify a file's SHA256 against a checksums file
fn verify_file_checksum(file_path: &Path, checksums_path: &Path, filename: &str) -> Result<()> {
    use sha2::{Sha256, Digest};
    
    // Read checksums file
    let checksums = fs::read_to_string(checksums_path)?;
    
    // Find expected checksum for this file
    let expected = checksums
        .lines()
        .find(|line| line.contains(filename))
        .and_then(|line| line.split_whitespace().next())
        .context(format!("No checksum found for {} in SHA256SUMS.txt", filename))?;
    
    // Calculate actual checksum
    let mut file = File::open(file_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    
    if actual != expected {
        bail!(
            "Checksum mismatch for {}!\n  Expected: {}\n  Got: {}\n\
            The download may be corrupted. Please try again.",
            filename, expected, actual
        );
    }
    
    debug!("Checksum verified for {}", filename);
    Ok(())
}

/// Extract boot files from tarball
fn extract_tarball(tarball_path: &Path, output_dir: &Path, arch: &str) -> Result<()> {
    let tarball = File::open(tarball_path)?;
    let decoder = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(decoder);
    
    // Files to extract from tarball (arch-prefixed paths like "aarch64-x1e/bzImage")
    let target_files = [
        "bzImage", "initrd", "init-path", "device.dtb",
        "platform", "default-device", "dtb-name", "SHA256SUMS"
    ];
    
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();
        
        // Security: Verify path is within expected arch directory (no path traversal)
        if path_str.contains("..") {
            warn!("Skipping suspicious path in tarball: {}", path_str);
            continue;
        }
        
        let filename = path.file_name()
            .map(|n| n.to_string_lossy().to_string());
        
        if let Some(name) = filename {
            // Only extract files that are in the correct arch directory and are expected files
            if path_str.contains(arch) && target_files.contains(&name.as_str()) {
                let dest = output_dir.join(&name);
                debug!("Extracting {} -> {:?}", path_str, dest);
                entry.unpack(&dest)?;
                
                // Verify extraction succeeded
                if !dest.exists() {
                    warn!("Failed to extract {}: file not created", name);
                }
            }
        }
    }
    
    Ok(())
}

/// Download a file and verify its SHA256 checksum
fn download_file_with_checksum(
    url: &str, 
    dir: &Path, 
    filename: &str,
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    let dest = dir.join(filename);
    
    debug!("Downloading {} -> {:?}", url, dest);
    
    // Use a client with extended timeouts for large files
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))  // 5 minute timeout for large downloads
        .connect_timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;
    
    let response = client.get(url).send()
        .with_context(|| format!("Failed to download {}", url))?;
    
    if !response.status().is_success() {
        bail!("Download failed with status: {}", response.status());
    }
    
    let bytes = response.bytes()?;
    
    // Verify SHA256 if provided
    if let Some(expected) = expected_sha256 {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = format!("{:x}", hasher.finalize());
        
        if actual != expected {
            bail!(
                "SHA256 mismatch for {}!\n  Expected: {}\n  Got: {}\n\
                This could indicate a corrupted download or tampering.",
                filename, expected, actual
            );
        }
        debug!("SHA256 verified for {}", filename);
    }
    
    let mut file = File::create(&dest)?;
    file.write_all(&bytes)?;
    
    info!("Downloaded {} ({} bytes)", filename, bytes.len());
    Ok(dest)
}

/// Extract specific EFI files from a .deb package
/// 
/// .deb structure:
/// - debian-binary (version)
/// - control.tar.* (metadata)
/// - data.tar.* (actual files)
fn extract_deb_efi_files(deb_path: &Path, output_dir: &Path, targets: &[&str]) -> Result<()> {
    debug!("Extracting {:?} from {:?}", targets, deb_path);
    
    let deb_file = File::open(deb_path)?;
    let mut archive = ar::Archive::new(deb_file);
    
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry?;
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        
        // We want data.tar.* (could be .xz, .zst, .gz)
        if name.starts_with("data.tar") {
            debug!("Found data archive: {}", name);
            
            // Read the entire data.tar.* into memory
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            
            // Decompress based on extension
            let decompressed = if name.ends_with(".xz") {
                decompress_xz(&data)?
            } else if name.ends_with(".zst") {
                decompress_zstd(&data)?
            } else if name.ends_with(".gz") {
                decompress_gzip(&data)?
            } else {
                // Assume uncompressed tar
                data
            };
            
            // Extract from tar
            extract_from_tar(&decompressed, output_dir, targets)?;
            return Ok(());
        }
    }
    
    bail!("No data.tar found in .deb package");
}

fn extract_from_tar(tar_data: &[u8], output_dir: &Path, targets: &[&str]) -> Result<()> {
    let cursor = Cursor::new(tar_data);
    let mut archive = tar::Archive::new(cursor);
    
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let filename = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        
        // Check if this is one of our target files
        if targets.iter().any(|t| filename == *t || filename.ends_with(t)) {
            let dest = output_dir.join(filename);
            debug!("Extracting {} -> {:?}", filename, dest);
            
            let mut file = File::create(&dest)?;
            std::io::copy(&mut entry, &mut file)?;
        }
    }
    
    Ok(())
}

fn decompress_xz(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::BufReader;
    
    let reader = BufReader::new(Cursor::new(data));
    let mut decompressor = xz2::read::XzDecoder::new(reader);
    let mut decompressed = Vec::new();
    decompressor.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

fn decompress_zstd(data: &[u8]) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    zstd::stream::copy_decode(Cursor::new(data), &mut decompressed)?;
    Ok(decompressed)
}

fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    
    let mut decoder = GzDecoder::new(Cursor::new(data));
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

/// Generate GRUB configuration for NixOS installation
/// 
/// This config is used for initial boot into the NixOS installer.
/// The kernel and initrd are loaded from ESP (where GRUB is), and the
/// loopback image is passed as a parameter for the installer to mount.
/// 
/// `init_path` is the path to the NixOS init binary (e.g., /nix/store/...-nixos-.../init)
/// `has_dtb` indicates if a Device Tree Blob should be loaded (required for ARM64 X1E)
pub fn generate_grub_config(nixos_root: &str, install_type: &str, init_path: Option<&str>, has_dtb: bool) -> String {
    // Build kernel parameters
    let mut params = Vec::new();
    
    // init= is required for NixOS to boot
    if let Some(init) = init_path {
        params.push(format!("init={}", init));
    }
    
    // For loopback install, we pass the disk location as kernel parameter
    // Quote the path in case it contains spaces
    let is_loopback = install_type == "loopback" || install_type == "quick";
    if is_loopback {
        let loopback_path = format!("{}/root.disk", nixos_root.replace("\\", "/"));
        // GRUB kernel params with spaces need quoting
        if loopback_path.contains(' ') {
            params.push(format!("nixos.loopback=\"{}\"", loopback_path));
        } else {
            params.push(format!("nixos.loopback={}", loopback_path));
        }
    }
    
    // X1E kernel parameters for Snapdragon hardware
    if has_dtb {
        params.push("pd_ignore_unused".to_string());
        params.push("clk_ignore_unused".to_string());
        // Force console to tty1 - without this, systemd may pick UART as console
        // and not display anything on screen (issue with Yoga Slim 7x)
        params.push("console=tty1".to_string());
    }
    
    let kernel_params = params.join(" ");

    // GRUB commands: linuxefi/initrdefi are x86_64-only (for certain Secure Boot setups)
    // ARM64 GRUB doesn't have these commands - must use linux/initrd
    // Ubuntu's signed GRUB works with linux/initrd on both architectures
    let (linux_cmd, initrd_cmd) = ("linux", "initrd");
    
    // For loopback installs, kernel/initrd/dtb are stored on the NTFS partition
    // alongside root.disk to avoid ESP space limitations (initrd can be 500MB+)
    // We use GRUB's search command to find the partition by looking for root.disk
    let boot_path = if is_loopback {
        // Convert Windows path (C:\NixOS) to GRUB path (/NixOS/boot)
        let grub_root = nixos_root.replace("\\", "/");
        // Strip drive letter if present (C:/NixOS -> /NixOS)
        let grub_root = if grub_root.chars().nth(1) == Some(':') {
            &grub_root[2..]
        } else {
            &grub_root
        };
        format!("{}/boot", grub_root)
    } else {
        "/EFI/NixOS".to_string()
    };
    
    // Prefix for boot files - use $ntfsroot variable for loopback
    let file_prefix = if is_loopback {
        "($ntfsroot)".to_string()
    } else {
        String::new()
    };
    
    // Device Tree Blob command for ARM64 X1E - must come BEFORE linux command
    let dtb_cmd = if has_dtb {
        format!("    devicetree {}{}/device.dtb\n", file_prefix, boot_path)
    } else {
        String::new()
    };
    
    // For loopback, we need to search for and set the NTFS partition
    let search_cmd = if is_loopback {
        let search_file = format!("{}/root.disk", nixos_root.replace("\\", "/"));
        let search_file = if search_file.chars().nth(1) == Some(':') {
            &search_file[2..]
        } else {
            &search_file
        };
        format!(r#"
# Search for the partition containing NixOS boot files
# This finds the NTFS partition with root.disk
search --no-floppy --file {} --set=ntfsroot
"#, search_file)
    } else {
        String::new()
    };

    format!(r#"# NixOS Easy Install - GRUB Configuration
# Auto-generated - do not edit manually

set timeout=5
set default=0

# Load required modules
insmod part_gpt
insmod fat
insmod ntfs
insmod ext2
insmod loopback
insmod normal
insmod linux
insmod all_video
insmod search
{search_cmd}
menuentry "NixOS Installer" --class nixos --class gnu-linux --class os {{
{dtb_cmd}    # Load kernel and initrd
    {linux_cmd} {file_prefix}{boot_path}/bzImage {kernel_params} quiet
    {initrd_cmd} {file_prefix}{boot_path}/initrd
}}

menuentry "NixOS Installer (verbose)" --class nixos --class gnu-linux --class os {{
{dtb_cmd}    {linux_cmd} {file_prefix}{boot_path}/bzImage {kernel_params}
    {initrd_cmd} {file_prefix}{boot_path}/initrd
}}

menuentry "Windows Boot Manager" --class windows {{
    insmod chain
    chainloader /EFI/Microsoft/Boot/bootmgfw.efi
}}

menuentry "UEFI Firmware Settings" {{
    fwsetup
}}
"#, 
        linux_cmd = linux_cmd,
        initrd_cmd = initrd_cmd,
        kernel_params = kernel_params,
        dtb_cmd = dtb_cmd,
        search_cmd = search_cmd,
        file_prefix = file_prefix,
        boot_path = boot_path,
    )
}

/// Verify the integrity of downloaded assets
pub fn verify_assets(assets: &BootAssets) -> Result<()> {
    // Determine expected names based on architecture
    let (shim_name, grub_name) = if assets.arch == "aarch64" {
        ("shimaa64.efi", "grubaa64.efi")
    } else {
        ("shimx64.efi", "grubx64.efi")
    };
    
    // Check files exist and have reasonable sizes
    // These ranges are based on Ubuntu 24.04 packages
    let checks = [
        (&assets.shim, shim_name, 800_000, 2_500_000),   // shim varies by arch
        (&assets.grub, grub_name, 1_500_000, 5_000_000), // grub varies by arch
    ];
    
    for (path, name, min_size, max_size) in checks {
        if !path.exists() {
            bail!("{} not found at {:?}", name, path);
        }
        
        let size = fs::metadata(path)?.len();
        if size < min_size as u64 {
            bail!(
                "{} is too small ({} bytes, minimum expected {}). File may be corrupted or download incomplete.",
                name, size, min_size
            );
        }
        if size > max_size as u64 {
            warn!("{} is larger than expected ({} bytes, expected max {}). This may indicate a version change.",
                  name, size, max_size);
        }
        
        debug!("{} verified: {} bytes", name, size);
    }
    
    info!("Boot asset verification passed for {}", assets.arch);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_grub_config_generation() {
        let config = generate_grub_config("/NixOS", "loopback", Some("/nix/store/test-init"), false);
        assert!(config.contains("NixOS Installer"));
        assert!(config.contains("nixos.loopback=/NixOS/root.disk"));
        assert!(config.contains("init=/nix/store/test-init"));
        // Loopback uses NTFS partition, not ESP
        assert!(config.contains("/NixOS/boot/bzImage"));
        assert!(config.contains("search --no-floppy --file /NixOS/root.disk"));
        assert!(config.contains("Windows Boot Manager"));
    }
    
    #[test]
    fn test_grub_config_partition() {
        let config = generate_grub_config("/", "partition", None, false);
        // Partition install doesn't have loopback parameter
        assert!(!config.contains("nixos.loopback"));
        // Partition install uses ESP
        assert!(config.contains("/EFI/NixOS/bzImage"));
    }
    
    #[test]
    fn test_grub_config_with_init() {
        let config = generate_grub_config("/", "partition", Some("/nix/store/abc-nixos/init"), false);
        assert!(config.contains("init=/nix/store/abc-nixos/init"));
    }
    
    #[test]
    fn test_grub_config_with_dtb() {
        let config = generate_grub_config("/NixOS", "loopback", Some("/nix/store/test-init"), true);
        // DTB goes to boot folder on NTFS for loopback
        assert!(config.contains("devicetree ($ntfsroot)/NixOS/boot/device.dtb"));
        assert!(config.contains("pd_ignore_unused"));
        assert!(config.contains("clk_ignore_unused"));
    }
    
    #[test]
    fn test_platform_detection_logic() {
        // Test that X1E platforms require DTB
        assert!(HardwarePlatform::SnapdragonX1E.needs_custom_kernel());
        assert!(!HardwarePlatform::X86_64.needs_custom_kernel());
        assert!(!HardwarePlatform::Aarch64.needs_custom_kernel());
    }
    
    #[test]
    fn test_platform_arch_strings() {
        assert_eq!(HardwarePlatform::SnapdragonX1E.arch_string(), "aarch64-x1e");
        assert_eq!(HardwarePlatform::Aarch64.arch_string(), "aarch64");
        assert_eq!(HardwarePlatform::X86_64.arch_string(), "x86_64");
    }
    
    #[test]
    fn test_platform_base_arch() {
        assert_eq!(HardwarePlatform::SnapdragonX1E.base_arch(), "aarch64");
        assert_eq!(HardwarePlatform::Aarch64.base_arch(), "aarch64");
        assert_eq!(HardwarePlatform::X86_64.base_arch(), "x86_64");
    }
}
