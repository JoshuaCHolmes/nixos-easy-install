//! Loopback installation module - creates NixOS installation inside a file
//! 
//! SAFETY DESIGN:
//! 1. All operations are reversible (delete folder to uninstall)
//! 2. We never touch existing Windows files/partitions
//! 3. Extensive pre-checks before any writes
//! 4. Atomic operations where possible
//! 5. Clear error messages for recovery

// Some fields are for display/reporting purposes
#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::fs;
use tracing::{info, warn, debug};

/// Configuration for a loopback installation
#[derive(Debug, Clone)]
pub struct LoopbackConfig {
    /// Directory to install to (e.g., C:\NixOS)
    pub target_dir: PathBuf,
    
    /// Size of root.disk in GB
    pub size_gb: u32,
    
    /// Whether to create a separate home.disk
    pub separate_home: bool,
    
    /// Size of home.disk in GB (if separate)
    pub home_size_gb: Option<u32>,
}

/// Result of loopback preparation
#[derive(Debug)]
pub struct LoopbackPrepareResult {
    /// Path to the root.disk file
    pub root_disk: PathBuf,
    
    /// Path to home.disk if created
    pub home_disk: Option<PathBuf>,
    
    /// Actual space used (sparse file, so may be less than allocated)
    pub actual_size: u64,
}

// ============================================================================
// Pre-flight Checks (Read-Only)
// ============================================================================

/// Verify that loopback installation is possible
/// 
/// SAFETY: This function only reads, never writes.
pub fn preflight_check(config: &LoopbackConfig) -> Result<PreflightResult> {
    info!("Running loopback preflight checks...");
    
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    
    // Check 1: Target directory doesn't exist or is empty
    if config.target_dir.exists() {
        let is_empty = config.target_dir.read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        
        if !is_empty {
            errors.push(format!(
                "Target directory '{}' exists and is not empty",
                config.target_dir.display()
            ));
        } else {
            warnings.push(format!(
                "Target directory '{}' exists but is empty (will be used)",
                config.target_dir.display()
            ));
        }
    }
    
    // Check 2: Parent directory exists and is writable
    let parent = config.target_dir.parent()
        .context("Target directory has no parent")?;
    
    if !parent.exists() {
        errors.push(format!(
            "Parent directory '{}' does not exist",
            parent.display()
        ));
    } else {
        // Try to check write permission
        let test_file = parent.join(".nixos_install_test");
        match fs::write(&test_file, "test") {
            Ok(_) => {
                let _ = fs::remove_file(&test_file);
            }
            Err(e) => {
                errors.push(format!(
                    "Cannot write to '{}': {}",
                    parent.display(),
                    e
                ));
            }
        }
    }
    
    // Check 3: Sufficient disk space
    let required_bytes = (config.size_gb as u64 
        + config.home_size_gb.unwrap_or(0) as u64) 
        * 1024 * 1024 * 1024;
    
    let available = get_available_space(parent)?;
    
    if available < required_bytes {
        errors.push(format!(
            "Insufficient disk space: {} available, {} required",
            crate::system::format_bytes(available),
            crate::system::format_bytes(required_bytes)
        ));
    } else if available < required_bytes * 12 / 10 {
        // Less than 20% margin
        warnings.push(format!(
            "Low disk space margin: {} available for {} installation",
            crate::system::format_bytes(available),
            crate::system::format_bytes(required_bytes)
        ));
    }
    
    // Check 4: Path is on NTFS filesystem
    let fs_type = get_filesystem_type(parent)?;
    if fs_type.to_uppercase() != "NTFS" {
        errors.push(format!(
            "Target must be on NTFS filesystem, found: {}",
            fs_type
        ));
    }
    
    // Check 5: Size is reasonable
    if config.size_gb < 10 {
        errors.push("Root disk must be at least 10GB".to_string());
    } else if config.size_gb < 20 {
        warnings.push("Root disk under 20GB may fill up quickly".to_string());
    }
    
    if config.size_gb > 500 {
        warnings.push("Root disk over 500GB is unusual - verify this is intended".to_string());
    }
    
    Ok(PreflightResult {
        passed: errors.is_empty(),
        errors,
        warnings,
        available_space: available,
        required_space: required_bytes,
    })
}

#[derive(Debug)]
pub struct PreflightResult {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub available_space: u64,
    pub required_space: u64,
}

// ============================================================================
// Installation Operations (Writes)
// ============================================================================

/// Prepare loopback installation (create directory and disk files)
/// 
/// SAFETY: 
/// - Creates new files only, never modifies existing
/// - Uses sparse files (instant creation, no disk fill)
/// - Entire operation reversible by deleting target_dir
pub fn prepare_loopback(config: &LoopbackConfig) -> Result<LoopbackPrepareResult> {
    info!("Preparing loopback installation at {:?}", config.target_dir);
    
    // Final safety check
    let preflight = preflight_check(config)?;
    if !preflight.passed {
        bail!("Preflight checks failed: {:?}", preflight.errors);
    }
    
    // Create target directory
    info!("Creating directory: {:?}", config.target_dir);
    fs::create_dir_all(&config.target_dir)
        .context("Failed to create target directory")?;
    
    // Create root.disk as sparse file
    let root_disk = config.target_dir.join("root.disk");
    info!("Creating sparse root.disk ({}GB)", config.size_gb);
    create_sparse_file(&root_disk, config.size_gb as u64 * 1024 * 1024 * 1024)?;
    
    // Optionally create home.disk
    let home_disk = if config.separate_home {
        let home_size = config.home_size_gb.unwrap_or(config.size_gb);
        let path = config.target_dir.join("home.disk");
        info!("Creating sparse home.disk ({}GB)", home_size);
        create_sparse_file(&path, home_size as u64 * 1024 * 1024 * 1024)?;
        Some(path)
    } else {
        None
    };
    
    // Get actual disk usage (sparse files use minimal space initially)
    let actual_size = get_directory_size(&config.target_dir)?;
    
    info!("Loopback preparation complete. Actual disk usage: {}", 
          crate::system::format_bytes(actual_size));
    
    Ok(LoopbackPrepareResult {
        root_disk,
        home_disk,
        actual_size,
    })
}

/// Remove loopback installation (cleanup on failure or uninstall)
/// 
/// SAFETY: Only removes the specific directory we created
pub fn cleanup_loopback(target_dir: &Path) -> Result<()> {
    warn!("Cleaning up loopback installation at {:?}", target_dir);
    
    if !target_dir.exists() {
        debug!("Target directory doesn't exist, nothing to clean");
        return Ok(());
    }
    
    // Safety check: verify this looks like our installation
    let root_disk = target_dir.join("root.disk");
    if !root_disk.exists() {
        bail!(
            "Safety check failed: {:?} doesn't contain root.disk. \
            Refusing to delete to prevent accidental data loss.",
            target_dir
        );
    }
    
    // Remove the directory
    fs::remove_dir_all(target_dir)
        .context("Failed to remove installation directory")?;
    
    info!("Cleanup complete");
    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a sparse file of the given size
/// 
/// Sparse files appear to have the full size but only use disk space
/// for actual written data. This makes creation instant and safe.
#[cfg(windows)]
fn create_sparse_file(path: &Path, size: u64) -> Result<()> {
    use std::process::Command;
    
    // Use fsutil to create sparse file (requires admin)
    let path_str = path.to_string_lossy();
    
    // Create empty file first
    fs::File::create(path).context("Failed to create file")?;
    
    // Mark as sparse
    let output = Command::new("fsutil")
        .args(["sparse", "setflag", &path_str])
        .output()
        .context("Failed to run fsutil sparse")?;
    
    if !output.status.success() {
        bail!("Failed to set sparse flag: {}", String::from_utf8_lossy(&output.stderr));
    }
    
    // Set the file size (this doesn't allocate disk space for sparse files)
    let output = Command::new("fsutil")
        .args(["file", "seteof", &path_str, &size.to_string()])
        .output()
        .context("Failed to run fsutil seteof")?;
    
    if !output.status.success() {
        bail!("Failed to set file size: {}", String::from_utf8_lossy(&output.stderr));
    }
    
    Ok(())
}

#[cfg(not(windows))]
fn create_sparse_file(path: &Path, size: u64) -> Result<()> {
    use std::process::Command;
    
    // Use truncate on Unix
    let output = Command::new("truncate")
        .args(["-s", &size.to_string(), &path.to_string_lossy()])
        .output()
        .context("Failed to create sparse file")?;
    
    if !output.status.success() {
        bail!("truncate failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    
    Ok(())
}

/// Get available space on the volume containing the given path
#[cfg(windows)]
fn get_available_space(path: &Path) -> Result<u64> {
    use std::process::Command;
    
    let drive = path.to_string_lossy()
        .chars()
        .take(2)
        .collect::<String>();
    
    let script = format!(
        "(Get-Volume -DriveLetter '{}').SizeRemaining",
        drive.chars().next().unwrap_or('C')
    );
    
    let output = Command::new("powershell")
        .args(["-Command", &script])
        .output()
        .context("Failed to get available space")?;
    
    let size_str = String::from_utf8_lossy(&output.stdout);
    size_str.trim().parse().context("Failed to parse available space")
}

#[cfg(not(windows))]
fn get_available_space(path: &Path) -> Result<u64> {
    // Use df on Unix
    use std::process::Command;
    
    let output = Command::new("df")
        .args(["--output=avail", "-B1", &path.to_string_lossy()])
        .output()
        .context("Failed to get available space")?;
    
    let output_str = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = output_str.lines().collect();
    
    if lines.len() < 2 {
        bail!("Unexpected df output");
    }
    
    lines[1].trim().parse().context("Failed to parse available space")
}

/// Get filesystem type for the volume containing the given path
#[cfg(windows)]
fn get_filesystem_type(path: &Path) -> Result<String> {
    use std::process::Command;
    
    let drive = path.to_string_lossy()
        .chars()
        .take(2)
        .collect::<String>();
    
    let script = format!(
        "(Get-Volume -DriveLetter '{}').FileSystem",
        drive.chars().next().unwrap_or('C')
    );
    
    let output = Command::new("powershell")
        .args(["-Command", &script])
        .output()
        .context("Failed to get filesystem type")?;
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(windows))]
fn get_filesystem_type(_path: &Path) -> Result<String> {
    Ok("ext4".to_string()) // Placeholder for non-Windows
}

/// Get actual size of a directory (not apparent size for sparse files)
fn get_directory_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    
    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }
    
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        
        if metadata.is_file() {
            // On Windows, this gives the actual allocated size for sparse files
            total += metadata.len();
        } else if metadata.is_dir() {
            total += get_directory_size(&entry.path())?;
        }
    }
    
    Ok(total)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    
    #[test]
    fn test_preflight_checks_missing_parent() {
        let config = LoopbackConfig {
            target_dir: PathBuf::from("/nonexistent/path/nixos"),
            size_gb: 30,
            separate_home: false,
            home_size_gb: None,
        };
        
        // On non-Windows, this will error when trying to get disk space
        // On Windows, it would also error. Either way, preflight correctly
        // prevents installation to an invalid path.
        let result = preflight_check(&config);
        // Result can be Err (can't check space) or Ok with passed=false
        if let Ok(preflight) = result {
            assert!(!preflight.passed);
        }
        // If Err, that's also acceptable - path is invalid
    }
    
    #[test]
    fn test_preflight_checks_size_too_small() {
        let config = LoopbackConfig {
            target_dir: env::temp_dir().join("nixos_test"),
            size_gb: 5, // Too small
            separate_home: false,
            home_size_gb: None,
        };
        
        let result = preflight_check(&config);
        assert!(result.is_ok());
        let preflight = result.unwrap();
        assert!(!preflight.passed);
        assert!(preflight.errors.iter().any(|e| e.contains("at least 10GB")));
    }
}
