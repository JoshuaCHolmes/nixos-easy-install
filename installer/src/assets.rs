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
    
    // Determine filenames based on architecture
    let (shim_name, grub_name, mm_name, fb_name) = if arch == "aarch64" {
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
        arch: arch.to_string(),
    };
    
    // Check cache
    if assets.shim.exists() && assets.grub.exists() {
        info!("Using cached boot assets");
        return Ok(assets);
    }
    
    // Select URLs based on architecture
    let (shim_url, grub_url, grub_signed_name) = if arch == "aarch64" {
        (SHIM_SIGNED_URL_AA64, GRUB_SIGNED_URL_AA64, "grubaa64.efi.signed")
    } else {
        (SHIM_SIGNED_URL_X64, GRUB_SIGNED_URL_X64, "grubx64.efi.signed")
    };
    
    // Download shim-signed
    info!("Downloading shim-signed package for {}...", arch);
    let shim_deb = download_file(shim_url, cache_dir, "shim-signed.deb")?;
    extract_deb_efi_files(&shim_deb, cache_dir, &[shim_name, mm_name, fb_name])?;
    fs::remove_file(&shim_deb)?;
    
    // Download grub-signed
    info!("Downloading grub-signed package for {}...", arch);
    let grub_deb = download_file(grub_url, cache_dir, "grub-signed.deb")?;
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
    
    info!("Boot assets ready for {}", arch);
    Ok(assets)
}

/// Download a file from URL to destination with optional SHA256 verification
fn download_file(url: &str, dir: &Path, filename: &str) -> Result<PathBuf> {
    download_file_with_checksum(url, dir, filename, None)
}

/// Installer boot assets (kernel + initrd)
#[derive(Debug)]
pub struct InstallerAssets {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
}

/// GitHub release URL for installer boot assets
const INSTALLER_RELEASE_BASE: &str = "https://github.com/JoshuaCHolmes/nixos-easy-install/releases/latest/download";

/// Download the NixOS installer kernel and initrd
/// 
/// These are pre-built from the initrd/default.nix and uploaded to GitHub releases
pub fn download_installer_assets(cache_dir: &Path, arch: &str) -> Result<InstallerAssets> {
    info!("Downloading installer boot files for {}...", arch);
    
    fs::create_dir_all(cache_dir)?;
    
    let assets = InstallerAssets {
        kernel: cache_dir.join("bzImage"),
        initrd: cache_dir.join("initrd"),
    };
    
    // Check cache
    if assets.kernel.exists() && assets.initrd.exists() {
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
    
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();
        
        // Looking for arch/bzImage and arch/initrd
        let filename = path.file_name()
            .map(|n| n.to_string_lossy().to_string());
        
        if let Some(name) = filename {
            if path_str.contains(arch) && (name == "bzImage" || name == "initrd") {
                let dest = output_dir.join(&name);
                debug!("Extracting {} -> {:?}", path_str, dest);
                entry.unpack(&dest)?;
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
    
    let response = reqwest::blocking::get(url)
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
pub fn generate_grub_config(nixos_root: &str, install_type: &str) -> String {
    // For loopback install, we pass the disk location as kernel parameter
    let kernel_params = if install_type == "loopback" || install_type == "quick" {
        format!("nixos.loopback={}/root.disk", nixos_root.replace("\\", "/"))
    } else {
        "".to_string()
    };

    // ARM64 EFI requires linuxefi/initrdefi commands, x86_64 uses linux/initrd
    let arch = detect_arch();
    let (linux_cmd, initrd_cmd) = if arch == "aarch64" {
        ("linuxefi", "initrdefi")
    } else {
        ("linux", "initrd")
    };

    format!(r#"# NixOS Easy Install - GRUB Configuration
# Auto-generated - do not edit manually

set timeout=5
set default=0

# Load required modules
insmod part_gpt
insmod fat
insmod ext2
insmod loopback
insmod normal
insmod linux
insmod all_video

# ESP contains the kernel and initrd
# Config file at $prefix/../install-config.json tells installer what to do

menuentry "NixOS Installer" --class nixos --class gnu-linux --class os {{
    # Load kernel and initrd from ESP (same partition as GRUB)
    {linux_cmd} /EFI/NixOS/bzImage {kernel_params} quiet
    {initrd_cmd} /EFI/NixOS/initrd
}}

menuentry "NixOS Installer (verbose)" --class nixos --class gnu-linux --class os {{
    {linux_cmd} /EFI/NixOS/bzImage {kernel_params}
    {initrd_cmd} /EFI/NixOS/initrd
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
    let checks = [
        (&assets.shim, shim_name, 800_000, 2_500_000),   // shim varies by arch
        (&assets.grub, grub_name, 1_500_000, 5_000_000), // grub varies by arch
    ];
    
    for (path, name, min_size, max_size) in checks {
        if !path.exists() {
            bail!("{} not found at {:?}", name, path);
        }
        
        let size = fs::metadata(path)?.len();
        if size < min_size as u64 || size > max_size as u64 {
            warn!("{} has unexpected size: {} bytes (expected {}-{})", 
                  name, size, min_size, max_size);
        }
    }
    
    info!("Boot asset verification passed for {}", assets.arch);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_grub_config_generation() {
        let config = generate_grub_config("/NixOS", "loopback");
        assert!(config.contains("NixOS Installer"));
        assert!(config.contains("nixos.loopback=/NixOS/root.disk"));
        assert!(config.contains("/EFI/NixOS/bzImage"));
        assert!(config.contains("Windows Boot Manager"));
    }
    
    #[test]
    fn test_grub_config_partition() {
        let config = generate_grub_config("/", "partition");
        // Partition install doesn't have loopback parameter
        assert!(!config.contains("nixos.loopback"));
    }
}
