# Testing Checklist

## Pre-requisites

### Build Environment
- [ ] Access to x86_64-linux system (for Windows cross-compilation)
- [ ] Nix with flakes enabled
- [ ] Working network connection

### Test Environment
- [ ] Windows 10/11 machine with:
  - [ ] UEFI boot mode
  - [ ] Secure Boot enabled (recommended)
  - [ ] At least 50GB free disk space
  - [ ] Administrator access
- [ ] Backup of important data (always!)

## Build Tests

### Native Build (Linux)
```bash
cd installer
cargo build --release
```
- [ ] Compiles without errors
- [ ] No warnings (or only expected `dead_code` from reserved features)

### Cross-Compilation (Windows target)
```bash
# On x86_64-linux
nix develop
cargo build --release --target x86_64-pc-windows-gnu
```
- [ ] Cross-compilation succeeds
- [ ] Binary is valid Windows executable

### Initrd Build
```bash
nix build .#initrd
```
- [ ] Build completes
- [ ] Output contains bootable Linux system

## Windows GUI Tests

### Startup
- [ ] Application starts without crash
- [ ] UAC prompt appears and works
- [ ] Window appears at correct size (800x600)

### System Detection
- [ ] UEFI/Legacy mode detected correctly
- [ ] Secure Boot status detected
- [ ] ESP found and reported
- [ ] Memory size correct
- [ ] Disk information displayed
- [ ] Validation passes/fails appropriately

### Wizard Flow
- [ ] Can navigate forward and back
- [ ] Progress indicator updates
- [ ] All 7 steps accessible

### Configuration
- [ ] Hostname validation works (rejects invalid)
- [ ] Username validation works (rejects reserved names)
- [ ] Password confirmation matches
- [ ] Disk size slider functions
- [ ] Config source selection works

### Dry Run
- [ ] "Test (Dry Run)" button works
- [ ] Network connectivity checked
- [ ] Storage space validated
- [ ] No actual changes made

## Installation Tests

### Loopback (Quick Install)

#### Pre-Installation
- [ ] Target directory created (C:\NixOS)
- [ ] root.disk created as sparse file
- [ ] Correct apparent size (user's choice)
- [ ] Actual disk usage minimal (~0 initially)

#### Bootloader Setup
- [ ] EFI\NixOS folder created
- [ ] shimx64.efi present
- [ ] grubx64.efi present
- [ ] grub.cfg present
- [ ] install-config.json present
- [ ] UEFI boot entry created
- [ ] Boot entry verifiable in bcdedit

#### Switching Utilities
- [ ] SwitchOS folder created
- [ ] boot-to-nixos.bat present
- [ ] boot-to-nixos.ps1 present
- [ ] create-shortcut.ps1 present

### Reboot & NixOS Installation

#### Boot Process
- [ ] GRUB menu appears
- [ ] NixOS Install entry present
- [ ] Windows entry present
- [ ] Selecting NixOS Install proceeds

#### Unattended Installer
- [ ] ESP mounted correctly
- [ ] install-config.json read
- [ ] Windows NTFS partition found
- [ ] root.disk mounted
- [ ] Hardware detection runs
- [ ] Correct hardware modules selected (ThinkPad, Framework, etc.)
- [ ] Flake cloned successfully
- [ ] nixos-install runs to completion
- [ ] System reboots automatically

### Post-Installation

#### First Boot
- [ ] GRUB menu appears
- [ ] NixOS entry present
- [ ] Windows entry present
- [ ] Booting NixOS works
- [ ] Login with created user works
- [ ] Network functional

#### Laptop Features (if applicable)
- [ ] setup-hibernate service ran (or can be run manually)
- [ ] Hibernate configuration in hardware-configuration.nix
- [ ] `systemctl hibernate` works
- [ ] Lid close behavior correct
- [ ] Touchpad works

#### OS Switching
- [ ] From NixOS: `boot-to-windows` sets next boot
- [ ] From Windows: desktop shortcut or script works
- [ ] Both OSes remain bootable

## Rollback Tests

### Installation Failure Recovery
- [ ] If installation fails mid-way, cleanup runs
- [ ] Boot entry removed
- [ ] ESP folder removed
- [ ] Loopback folder removed
- [ ] System bootable to Windows

### Uninstallation
1. Boot to Windows
2. Delete C:\NixOS folder
3. Run: `bcdedit /delete {guid}`
4. Delete EFI\NixOS folder
- [ ] System boots normally to Windows
- [ ] No traces of NixOS remain

## Edge Cases

### Secure Boot Variations
- [ ] Works with Secure Boot enabled
- [ ] Works with Secure Boot disabled
- [ ] No MOK enrollment required

### Multiple Disks
- [ ] Correct disk selected for loopback
- [ ] ESP found on correct disk

### Existing NixOS
- [ ] Warning shown if NixOS folder exists
- [ ] Can reinstall over existing

### Low Disk Space
- [ ] Warning at <20% margin
- [ ] Error if insufficient space

### Network Issues
- [ ] Graceful handling of download failures
- [ ] Offline mode works with cached assets

## Security Verification

- [ ] Password hash uses SHA-512 crypt
- [ ] Password hash works with NixOS hashedPassword
- [ ] install-config.json removed after install
- [ ] No plaintext passwords stored anywhere
- [ ] All downloads over HTTPS
- [ ] SHA256 verification of boot assets

## Performance Notes

- [ ] Sparse file creation is instant
- [ ] GUI responsive during system detection
- [ ] Progress updates during installation
- [ ] No excessive CPU/memory usage
