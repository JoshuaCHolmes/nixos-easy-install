# NixOS Easy Install

A Windows application that installs NixOS alongside Windows with minimal user intervention.

## Features

### ✅ Implemented

- **Graphical Installer** - 7-step wizard with validation at each stage
- **Quick Install (Loopback)** - No partition changes required, uses disk image on NTFS
- **Secure Boot Support** - Uses Ubuntu's signed shim/GRUB chain (no MOK enrollment needed)
- **Hardware Auto-Detection**:
  - CPU model and generation (Intel 11th-14th Gen, AMD Ryzen 5000-8000)
  - RAM size for automatic swap sizing
  - GPU detection (NVIDIA, AMD, Intel with bus IDs)
  - WiFi chipset identification
  - Laptop vs desktop detection
  - Known hardware matching (ThinkPad, Framework, Dell XPS, ASUS ROG, System76, etc.)
- **Power Management** (for laptops):
  - TLP with battery/AC profiles
  - Hibernate with suspend-then-hibernate
  - Deep sleep (S3) preference
  - Battery charge thresholds
- **OS Switching**:
  - GRUB boot menu with Windows entry
  - Desktop shortcut for "Boot to NixOS" from Windows
  - CLI commands for boot switching from NixOS
- **Configuration Options**:
  - Starter Config - Modular, beginner-friendly defaults
  - Minimal Config - Bare NixOS for experienced users
  - Custom URL - Bring your own flake

### 🚧 In Progress

- Full partition installation (currently loopback only)
- Cross-compilation testing on x86_64 hardware
- Real Windows hardware testing

## How It Works

1. **Run Installer on Windows**: Download and run the GUI installer
2. **Choose Options**: Select install type, configuration, and create user
3. **Automatic Setup**: Installer creates disk image and configures bootloader
4. **Reboot**: System boots into NixOS installer initrd
5. **Unattended Install**: NixOS installs itself with detected hardware config
6. **Ready**: Boot into your new NixOS with GRUB dual-boot menu

## Project Structure

```
nixos-easy-install/
├── installer/           # Rust Windows application (egui GUI)
│   └── src/
│       ├── main.rs      # Entry point, admin elevation
│       ├── ui.rs        # 7-step wizard interface
│       ├── install.rs   # Installation orchestration
│       ├── system.rs    # Windows system detection (PowerShell)
│       ├── loopback.rs  # Sparse disk image creation
│       ├── bootloader.rs # ESP setup, UEFI boot entry
│       ├── assets.rs    # Ubuntu package downloading
│       ├── switching.rs # OS switching utilities
│       └── config.rs    # Configuration types & validation
├── initrd/              # NixOS installer system
│   └── default.nix      # Unattended installer with hardware detection
├── bootloader/          # (Reserved for Secure Boot assets)
├── configs/             # (Reserved for bundled configs)
└── docs/
```

## Companion Repository

This installer works with [nixos-starter-config](https://github.com/JoshuaCHolmes/nixos-starter-config), a modular NixOS configuration template featuring:

- Modular design with optional components
- Laptop power management (`jch.laptop`)
- Development environments (`jch.development`)
- GUI options (`jch.gui`)
- OS switching commands

## Building

Requires NixOS or Nix with flakes enabled.

```bash
# Enter development shell
nix develop

# Build Windows installer (requires x86_64-linux for cross-compilation)
cargo build --release --target x86_64-pc-windows-gnu

# Build installer initrd
nix build .#initrd

# Build everything
nix build
```

## Safety Design

- **No Partition Changes**: Loopback install doesn't modify Windows partitions
- **Reversible**: Uninstall by deleting folder and removing boot entry
- **Secure Boot**: Uses Microsoft-signed chain, no key enrollment
- **Validation**: Extensive pre-flight checks before any changes
- **Rollback**: Automatic cleanup on installation failure
- **Verified Downloads**: SHA256 checksums for all boot assets

## Development Status

🟡 **Alpha** - Core functionality implemented, needs testing on real hardware

## License

GPL-3.0 (compatible with NixOS and wubiuefi components)
