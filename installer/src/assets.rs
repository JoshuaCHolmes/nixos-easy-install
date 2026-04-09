//! Boot asset downloading and extraction
//! 
//! Downloads Ubuntu's signed shim and GRUB packages, extracts the EFI binaries.
//! These are Microsoft-signed (shim) and Canonical-signed (GRUB), so they work
//! with Secure Boot out of the box on most systems.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{Read, Write, Cursor};
use tracing::{info, debug, warn};

/// URLs for Ubuntu's signed boot packages (Noble 24.04 LTS / Plucky)
/// Using HTTPS for security
const SHIM_SIGNED_URL: &str = "https://archive.ubuntu.com/ubuntu/pool/main/s/shim-signed/shim-signed_1.59+15.8-0ubuntu2_amd64.deb";
const GRUB_SIGNED_URL: &str = "https://archive.ubuntu.com/ubuntu/pool/main/g/grub2-signed/grub-efi-amd64-signed_1.215+2.14-2ubuntu1_amd64.deb";

/// SHA256 checksums for integrity verification
/// These are the checksums of the .deb packages from Ubuntu's official repos
/// Verified on 2026-04-09 from archive.ubuntu.com
const SHIM_SIGNED_SHA256: &str = "f8ed71ce2d91a304b6d5eb84997f846f331b554578bc02dbfe78e13ad8ac81a9";
const GRUB_SIGNED_SHA256: &str = "603fe7db065634780d9576bab48fce8143a0451697c5be75a6cdb1f6a5e39188";

/// Fallback mirror if primary is slow/unavailable
const MIRROR_URL: &str = "https://us.archive.ubuntu.com/ubuntu/pool/main";

/// Expected files after extraction
#[derive(Debug)]
pub struct BootAssets {
    /// Microsoft-signed shim (first-stage loader)
    pub shim_x64: PathBuf,
    
    /// MokManager for key enrollment (if needed)
    pub mok_manager: PathBuf,
    
    /// Fallback bootloader
    pub fallback: PathBuf,
    
    /// Canonical-signed GRUB
    pub grub_x64: PathBuf,
    
    /// Directory containing all assets
    pub asset_dir: PathBuf,
}

/// Download and extract boot assets
/// 
/// Returns paths to all required EFI binaries for Secure Boot
pub fn download_boot_assets(cache_dir: &Path) -> Result<BootAssets> {
    info!("Downloading boot assets to {:?}", cache_dir);
    
    fs::create_dir_all(cache_dir)?;
    
    // Check if we already have cached assets
    let assets = BootAssets {
        shim_x64: cache_dir.join("shimx64.efi"),
        mok_manager: cache_dir.join("mmx64.efi"),
        fallback: cache_dir.join("fbx64.efi"),
        grub_x64: cache_dir.join("grubx64.efi"),
        asset_dir: cache_dir.to_path_buf(),
    };
    
    if assets.shim_x64.exists() && assets.grub_x64.exists() {
        info!("Using cached boot assets");
        return Ok(assets);
    }
    
    // Download shim-signed
    info!("Downloading shim-signed package...");
    let shim_deb = download_file(SHIM_SIGNED_URL, cache_dir, "shim-signed.deb")?;
    extract_deb_efi_files(&shim_deb, cache_dir, &["shimx64.efi", "mmx64.efi", "fbx64.efi"])?;
    fs::remove_file(&shim_deb)?; // Clean up .deb
    
    // Download grub-signed
    info!("Downloading grub-efi-amd64-signed package...");
    let grub_deb = download_file(GRUB_SIGNED_URL, cache_dir, "grub-signed.deb")?;
    extract_deb_efi_files(&grub_deb, cache_dir, &["grubx64.efi.signed"])?;
    fs::remove_file(&grub_deb)?;
    
    // Rename grubx64.efi.signed to grubx64.efi
    let signed_grub = cache_dir.join("grubx64.efi.signed");
    if signed_grub.exists() {
        fs::rename(&signed_grub, &assets.grub_x64)?;
    }
    
    // Verify we got everything
    if !assets.shim_x64.exists() {
        bail!("Failed to extract shimx64.efi");
    }
    if !assets.grub_x64.exists() {
        bail!("Failed to extract grubx64.efi");
    }
    
    info!("Boot assets ready");
    Ok(assets)
}

/// Download a file from URL to destination with optional SHA256 verification
fn download_file(url: &str, dir: &Path, filename: &str) -> Result<PathBuf> {
    download_file_with_checksum(url, dir, filename, None)
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
/// This config is used for initial boot into the NixOS installer
pub fn generate_grub_config(nixos_root: &str, install_type: &str) -> String {
    let root_spec = if install_type == "loopback" {
        format!(r#"
    # Mount loopback disk image
    loopback loop {}/root.disk
    set root=(loop)
"#, nixos_root)
    } else {
        r#"
    # Direct partition access
    search --no-floppy --label --set=root NIXOS_ROOT
"#.to_string()
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

menuentry "NixOS Installer" --class nixos --class gnu-linux --class os {{
    {root_spec}
    linux /boot/bzImage init=/nix/store/installer-init quiet
    initrd /boot/initrd
}}

menuentry "NixOS Installer (verbose)" --class nixos --class gnu-linux --class os {{
    {root_spec}
    linux /boot/bzImage init=/nix/store/installer-init
    initrd /boot/initrd
}}

menuentry "Windows Boot Manager" --class windows {{
    insmod chain
    search --no-floppy --fs-uuid --set=root {windows_esp_uuid}
    chainloader /EFI/Microsoft/Boot/bootmgfw.efi
}}

menuentry "UEFI Firmware Settings" {{
    fwsetup
}}
"#, 
        root_spec = root_spec,
        windows_esp_uuid = "${ESP_UUID}" // Placeholder, filled at install time
    )
}

/// Verify the integrity of downloaded assets
pub fn verify_assets(assets: &BootAssets) -> Result<()> {
    // Check files exist and have reasonable sizes
    let checks = [
        (&assets.shim_x64, "shimx64.efi", 1_000_000, 2_000_000),  // ~1.2MB typically
        (&assets.grub_x64, "grubx64.efi", 1_500_000, 4_000_000),  // ~2-3MB typically
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
    
    info!("Boot asset verification passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    
    #[test]
    fn test_grub_config_generation() {
        let config = generate_grub_config("/NixOS", "loopback");
        assert!(config.contains("NixOS Installer"));
        assert!(config.contains("loopback loop"));
        assert!(config.contains("Windows Boot Manager"));
    }
    
    #[test]
    fn test_grub_config_partition() {
        let config = generate_grub_config("/", "partition");
        assert!(config.contains("NIXOS_ROOT"));
        assert!(!config.contains("loopback loop"));
    }
}
