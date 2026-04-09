# Installer Initrd

This is a custom NixOS initrd that performs unattended installation.

## How It Works

1. Windows installer places `install-config.json` on the EFI System Partition
2. System boots into this initrd via the signed bootloader chain
3. Initrd reads the config and performs installation automatically
4. On completion, removes itself and reboots into the new system

## install-config.json Schema

```json
{
  "version": 1,
  "install_type": "loopback|partition",
  "hostname": "nixos",
  "username": "user",
  "password_hash": "$6$...",
  
  "flake": {
    "type": "starter|minimal|url|local",
    "url": "github:user/repo",
    "hostname": "configuration-name"
  },
  
  "loopback": {
    "target_dir": "C:\\NixOS",
    "size_gb": 64
  },
  
  "partition": {
    "root": "/dev/sda3",
    "boot": "/dev/sda1",
    "swap": "/dev/sda4"
  },
  
  "options": {
    "encrypt": false,
    "secure_boot": true
  }
}
```

## Build

```bash
nix build .#initrd
```

Output is a bootable initrd image that can be chainloaded from the signed GRUB.
