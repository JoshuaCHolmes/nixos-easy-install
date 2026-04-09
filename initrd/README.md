# NixOS Easy Install - Installer System

This directory contains the NixOS system that performs unattended installation.

## How It Works

1. Windows installer prepares everything and writes `install-config.json` to ESP
2. System reboots, GRUB loads this NixOS installer (via shim for Secure Boot)
3. Installer reads `/boot/efi/EFI/NixOS/install-config.json`
4. Mounts filesystems, fetches config, runs `nixos-install`
5. On success, cleans up and reboots into the new NixOS system

## Building

```bash
# Build the installer system
nix-build default.nix

# The result is a NixOS system toplevel
# Extract kernel and initrd for the Windows installer to use
```

## install-config.json Schema

```json
{
  "install_type": "loopback",  // or "partition"
  "hostname": "nixos",
  "username": "user",
  "password_hash": "$6$...",   // mkpasswd -m sha-512
  
  "loopback": {                // For loopback installs
    "target_dir": "C:\\NixOS",
    "size_gb": 64
  },
  
  "partition": {               // For partition installs
    "root": "/dev/nvme0n1p5",
    "boot": "/dev/nvme0n1p1",
    "swap": "/dev/nvme0n1p6"   // optional
  },
  
  "flake": {
    "type": "minimal",         // minimal, starter, url, local
    "url": "",                 // For type=url
    "hostname": "nixos"        // Which flake output to build
  }
}
```

## Flake Types

- **minimal**: Generates a basic working NixOS config
- **starter**: Clones a starter template with sensible defaults  
- **url**: Clones any git repo containing a flake
- **local**: Assumes config is already in place (advanced)

## Debugging

If installation fails, you'll be dropped to a bash shell.
Check `/tmp/install.log` for details.

Switch to tty2 (Alt+F2) during install to monitor progress.
