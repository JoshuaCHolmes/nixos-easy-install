//! System utilities - Windows API interactions

use anyhow::Result;

/// Check if the current process is running with administrator privileges
#[cfg(windows)]
pub fn is_admin() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::Foundation::HANDLE;
    
    unsafe {
        let mut token: HANDLE = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        
        if GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size,
            &mut size,
        ).is_err() {
            return false;
        }
        
        elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    // On non-Windows, check if running as root
    unsafe { libc::geteuid() == 0 }
}

/// Re-launch the application with administrator privileges
#[cfg(windows)]
pub fn elevate() -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    
    let exe = std::env::current_exe()?;
    
    // Use ShellExecuteW with "runas" verb to trigger UAC prompt
    Command::new("powershell")
        .args([
            "-Command",
            &format!(
                "Start-Process '{}' -Verb RunAs",
                exe.display()
            ),
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()?;
    
    std::process::exit(0);
}

#[cfg(not(windows))]
pub fn elevate() -> Result<()> {
    anyhow::bail!("Elevation not supported on this platform");
}

/// Get system information
pub struct SystemInfo {
    pub total_memory_gb: u64,
    pub available_disk_gb: u64,
    pub is_uefi: bool,
    pub secure_boot_enabled: bool,
}

#[cfg(windows)]
pub fn get_system_info() -> Result<SystemInfo> {
    // TODO: Implement using Windows APIs
    Ok(SystemInfo {
        total_memory_gb: 16,
        available_disk_gb: 100,
        is_uefi: true,
        secure_boot_enabled: true,
    })
}

#[cfg(not(windows))]
pub fn get_system_info() -> Result<SystemInfo> {
    Ok(SystemInfo {
        total_memory_gb: 16,
        available_disk_gb: 100,
        is_uefi: true,
        secure_boot_enabled: false,
    })
}

/// Get list of disk partitions
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub size_gb: u64,
    pub free_gb: u64,
    pub filesystem: String,
    pub is_system: bool,
}

pub fn get_disks() -> Result<Vec<DiskInfo>> {
    // TODO: Implement disk enumeration
    Ok(vec![
        DiskInfo {
            name: "C:".to_string(),
            size_gb: 500,
            free_gb: 200,
            filesystem: "NTFS".to_string(),
            is_system: true,
        },
    ])
}
