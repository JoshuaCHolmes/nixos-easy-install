//! OS Switching utilities
//!
//! Provides user-friendly mechanisms for switching between Windows and NixOS:
//!
//! 1. **GRUB Menu** (default after install):
//!    - 5-second timeout, NixOS default
//!    - Manual selection via arrow keys
//!
//! 2. **Windows "Boot to NixOS" shortcut**:
//!    - Desktop shortcut that sets next boot to NixOS
//!    - Uses bcdedit /bootsequence for one-time boot
//!
//! 3. **NixOS "Boot to Windows" command**:
//!    - CLI command: `boot-to-windows`
//!    - Uses efibootmgr --bootnext
//!
//! 4. **Keyboard shortcut approach** (optional):
//!    - Hold key during POST to override default
//!    - Varies by motherboard

// Some functions are installed/used in different OS contexts
#![allow(dead_code)]

use anyhow::{Context, Result};
use std::path::Path;
use std::fs;
use tracing::info;

/// Generate a Windows batch script that reboots into NixOS
pub fn generate_boot_to_nixos_script(nixos_boot_entry_id: &str) -> String {
    format!(r#"@echo off
REM Boot to NixOS - One-time reboot into NixOS
REM This sets NixOS as the next boot option, then reboots

echo Setting NixOS as next boot option...
bcdedit /bootsequence {{{entry_id}}}

if errorlevel 1 (
    echo Failed to set boot sequence. Are you running as Administrator?
    pause
    exit /b 1
)

echo.
echo Your computer will restart into NixOS.
echo Press any key to continue, or close this window to cancel...
pause >nul

shutdown /r /t 0
"#, entry_id = nixos_boot_entry_id)
}

/// Generate a PowerShell script for more robust boot switching
pub fn generate_boot_to_nixos_powershell(nixos_boot_entry_id: &str) -> String {
    format!(r#"# Boot to NixOS - PowerShell version
# Run as Administrator

$ErrorActionPreference = "Stop"

# Check for admin rights
$currentPrincipal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {{
    # Relaunch as admin
    Start-Process powershell "-ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
    exit
}}

Write-Host "Setting NixOS as next boot option..." -ForegroundColor Cyan

try {{
    $output = bcdedit /bootsequence {{{entry_id}}} 2>&1
    if ($LASTEXITCODE -ne 0) {{
        throw "bcdedit failed: $output"
    }}
    
    Write-Host "`nSuccess! Your computer will restart into NixOS." -ForegroundColor Green
    Write-Host "Press any key to reboot, or close this window to cancel..."
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    
    Restart-Computer -Force
}}
catch {{
    Write-Host "`nError: $_" -ForegroundColor Red
    Write-Host "Press any key to exit..."
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}}
"#, entry_id = nixos_boot_entry_id)
}

/// Generate a desktop shortcut (.lnk creation via PowerShell)
pub fn generate_desktop_shortcut_script() -> String {
    r#"# Create desktop shortcut for "Boot to NixOS"
$WshShell = New-Object -ComObject WScript.Shell
$Desktop = [Environment]::GetFolderPath("Desktop")
$Shortcut = $WshShell.CreateShortcut("$Desktop\Boot to NixOS.lnk")
$Shortcut.TargetPath = "$PSScriptRoot\boot-to-nixos.ps1"
$Shortcut.IconLocation = "%SystemRoot%\System32\shell32.dll,21"
$Shortcut.Description = "Reboot into NixOS"
$Shortcut.Save()

Write-Host "Created desktop shortcut: Boot to NixOS"
"#.to_string()
}

/// Generate a NixOS shell script to boot to Windows
pub fn generate_boot_to_windows_script() -> String {
    r#"#!/usr/bin/env bash
# Boot to Windows - One-time reboot into Windows
# This sets Windows as the next boot option, then reboots

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "This script requires root privileges."
    exec sudo "$0" "$@"
fi

echo "Finding Windows boot entry..."

# Find Windows Boot Manager entry
WINDOWS_ENTRY=$(efibootmgr | grep -i "windows" | head -1 | grep -oP 'Boot[0-9A-F]+' | sed 's/Boot//')

if [[ -z "$WINDOWS_ENTRY" ]]; then
    echo "Error: Could not find Windows Boot Manager entry"
    echo ""
    echo "Available boot entries:"
    efibootmgr
    exit 1
fi

echo "Setting Windows as next boot (entry $WINDOWS_ENTRY)..."
efibootmgr --bootnext "$WINDOWS_ENTRY"

echo ""
echo "Your computer will restart into Windows."
read -p "Press Enter to reboot, or Ctrl+C to cancel..."

systemctl reboot
"#.to_string()
}

/// Generate a NixOS module that provides the boot-to-windows command
pub fn generate_nixos_switching_module() -> String {
    r#"{ config, lib, pkgs, ... }:

# OS Switching utilities for dual-boot systems
# Provides easy commands to switch between Windows and NixOS

let
  boot-to-windows = pkgs.writeShellScriptBin "boot-to-windows" ''
    set -euo pipefail
    
    if [[ $EUID -ne 0 ]]; then
      echo "This command requires root privileges."
      exec sudo "$0" "$@"
    fi
    
    echo "Finding Windows boot entry..."
    
    WINDOWS_ENTRY=$(${pkgs.efibootmgr}/bin/efibootmgr | grep -i "windows" | head -1 | grep -oP 'Boot[0-9A-F]+' | sed 's/Boot//')
    
    if [[ -z "$WINDOWS_ENTRY" ]]; then
      echo "Error: Could not find Windows Boot Manager entry"
      echo ""
      echo "Available boot entries:"
      ${pkgs.efibootmgr}/bin/efibootmgr
      exit 1
    fi
    
    echo "Setting Windows as next boot (entry $WINDOWS_ENTRY)..."
    ${pkgs.efibootmgr}/bin/efibootmgr --bootnext "$WINDOWS_ENTRY"
    
    echo ""
    echo "Your computer will restart into Windows."
    read -p "Press Enter to reboot, or Ctrl+C to cancel..."
    
    systemctl reboot
  '';
  
  boot-menu = pkgs.writeShellScriptBin "boot-menu" ''
    # Quick boot menu - shows available options
    echo "Boot Options:"
    echo ""
    echo "  1) Stay on NixOS (default)"
    echo "  2) Reboot to Windows (one-time)"
    echo "  3) Reboot to UEFI/BIOS settings"
    echo "  4) Show all boot entries"
    echo ""
    read -p "Select [1-4]: " choice
    
    case $choice in
      2)
        exec ${boot-to-windows}/bin/boot-to-windows
        ;;
      3)
        if [[ $EUID -ne 0 ]]; then
          exec sudo systemctl reboot --firmware-setup
        else
          systemctl reboot --firmware-setup
        fi
        ;;
      4)
        ${pkgs.efibootmgr}/bin/efibootmgr -v
        ;;
      *)
        echo "Staying on NixOS."
        ;;
    esac
  '';

in {
  environment.systemPackages = [
    boot-to-windows
    boot-menu
    pkgs.efibootmgr
  ];
}
"#.to_string()
}

/// Install Windows switching utilities to a directory
pub fn install_windows_switching_utils(
    install_dir: &Path,
    nixos_boot_entry_id: &str,
) -> Result<()> {
    fs::create_dir_all(install_dir)
        .context("Failed to create switching utils directory")?;
    
    // Write batch script
    let batch_path = install_dir.join("boot-to-nixos.bat");
    fs::write(&batch_path, generate_boot_to_nixos_script(nixos_boot_entry_id))
        .context("Failed to write batch script")?;
    info!("Created {:?}", batch_path);
    
    // Write PowerShell script  
    let ps_path = install_dir.join("boot-to-nixos.ps1");
    fs::write(&ps_path, generate_boot_to_nixos_powershell(nixos_boot_entry_id))
        .context("Failed to write PowerShell script")?;
    info!("Created {:?}", ps_path);
    
    // Write shortcut creator
    let shortcut_path = install_dir.join("create-shortcut.ps1");
    fs::write(&shortcut_path, generate_desktop_shortcut_script())
        .context("Failed to write shortcut script")?;
    info!("Created {:?}", shortcut_path);
    
    // Write README
    let readme = r#"# NixOS Switching Utilities

## From Windows → NixOS

### Option 1: Desktop Shortcut (Recommended)
Run `create-shortcut.ps1` once to create a desktop shortcut.
Then just double-click "Boot to NixOS" whenever you want to switch.

### Option 2: Command Line
Run `boot-to-nixos.bat` or `boot-to-nixos.ps1` as Administrator.

### Option 3: GRUB Menu
Just reboot normally - you'll see the GRUB menu with NixOS options.

### Option 4: Hotkey (experimental)
Hold SHIFT during boot to automatically select Windows.
Hold CTRL to pause at the GRUB menu indefinitely.
NOTE: This may not work on all hardware (USB keyboards, fast boot, etc.)

## From NixOS → Windows

Run: `boot-to-windows`

Or for a menu: `boot-menu`

## GRUB Timeout

The GRUB menu shows for 5 seconds by default:
- NixOS is default (boots automatically after timeout)
- Use arrow keys to select Windows
- Press Enter to boot immediately

## Laptop Power Tips

If you experience battery drain during sleep on NixOS:
1. Enable the laptop power module: `jch.laptop.enable = true;`
2. This prefers S3 (deep) sleep over S2idle
3. Enables suspend-then-hibernate after 2 hours
4. Run `powertop` to identify power-hungry devices

See: https://wiki.nixos.org/wiki/Power_Management
"#;
    fs::write(install_dir.join("README.txt"), readme)
        .context("Failed to write README")?;
    
    Ok(())
}

/// GRUB configuration with optional hotkey switching
/// 
/// Hotkey switching uses GRUB's keystatus module to detect Shift/Ctrl/Alt:
/// - Hold Shift at boot → Boot Windows
/// - No key held → Boot NixOS (default)
/// 
/// NOTE: keystatus is NOT reliable on all hardware. It may fail silently on:
/// - Some UEFI implementations
/// - USB keyboards (not initialized early enough)
/// - Systems with "fast boot" enabled
/// 
/// We include it as a convenience but the GRUB menu is always available as fallback.
pub fn generate_enhanced_grub_config(
    nixos_root: &str,
    windows_esp_uuid: &str,
    timeout_seconds: u32,
    enable_hotkey_switching: bool,
) -> String {
    let hotkey_section = if enable_hotkey_switching {
        r#"
# ============================================================
# Hotkey Switching (experimental - may not work on all hardware)
# Hold SHIFT during boot to select Windows
# ============================================================
insmod keystatus

# Check if Shift is held - if so, boot Windows
# This may fail silently on some hardware (USB keyboards, fast boot, etc.)
if keystatus --shift; then
  set default="Windows"
  set timeout=1
fi

# Check if Ctrl is held - if so, show extended menu immediately  
if keystatus --ctrl; then
  set timeout=-1
fi
"#
    } else {
        ""
    };
    
    format!(r#"# NixOS GRUB Configuration
# Generated by NixOS Easy Install

set timeout={timeout}
set default=0
{hotkey_section}
# Visual improvements
insmod all_video
if loadfont /boot/grub/fonts/unicode.pf2 ; then
  insmod gfxterm
  set gfxmode=auto
  set gfxpayload=keep
  terminal_output gfxterm
fi

# Color scheme
set menu_color_normal=white/black
set menu_color_highlight=black/light-gray

# Load required modules
insmod part_gpt
insmod fat
insmod ext2
insmod loopback
insmod linux

# NixOS (default)
menuentry "NixOS" --class nixos --class gnu-linux --class os {{
    search --no-floppy --set=root --label NIXOS_ROOT 2>/dev/null || set root={nixos_root}
    linux /boot/bzImage init=/nix/store/current-system/init
    initrd /boot/initrd
}}

# NixOS (previous generation) - populated by NixOS rebuild
menuentry "NixOS (previous generation)" --class nixos --class gnu-linux --class os {{
    search --no-floppy --set=root --label NIXOS_ROOT 2>/dev/null || set root={nixos_root}
    linux /boot/bzImage.old init=/nix/store/current-system/init
    initrd /boot/initrd.old
}}

# Windows
menuentry "Windows" --class windows {{
    insmod chain
    search --no-floppy --fs-uuid --set=root {windows_uuid}
    chainloader /EFI/Microsoft/Boot/bootmgfw.efi
}}

# Utilities
menuentry "UEFI Firmware Settings" --class settings {{
    fwsetup
}}

menuentry "Reboot" --class reboot {{
    reboot
}}

menuentry "Shutdown" --class shutdown {{
    halt
}}
"#,
        timeout = timeout_seconds,
        nixos_root = nixos_root,
        windows_uuid = windows_esp_uuid
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_boot_to_nixos_script() {
        let script = generate_boot_to_nixos_script("abcd1234-5678-90ab-cdef-1234567890ab");
        assert!(script.contains("bcdedit /bootsequence"));
        assert!(script.contains("abcd1234"));
    }
    
    #[test]
    fn test_boot_to_windows_script() {
        let script = generate_boot_to_windows_script();
        assert!(script.contains("efibootmgr"));
        assert!(script.contains("--bootnext"));
    }
    
    #[test]
    fn test_nixos_module() {
        let module = generate_nixos_switching_module();
        assert!(module.contains("boot-to-windows"));
        assert!(module.contains("boot-menu"));
    }
}
