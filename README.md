# NixOS Easy Install

A Windows application that installs NixOS with minimal user intervention.

## Vision

Make NixOS accessible to Windows users through a simple installer that handles
all the complexity of dual-booting, bootloader configuration, and system setup.

## Installation Paths

### Quick Install (Loopback)
- No partition changes required
- Creates a virtual disk file on your Windows partition
- Easy to uninstall - just delete the folder
- Slight performance overhead
- Great for trying NixOS

### Full Install (Partition)
- Shrinks Windows partition to make room for NixOS
- Full native performance
- Proper dual-boot setup
- Recommended for daily use

## Configuration Options

1. **Starter Config** - Beginner-friendly setup with sensible defaults
2. **Minimal Config** - Bare NixOS, just enough to boot
3. **Custom Flake URL** - Bring your own configuration
4. **Local Flake Path** - Use an existing local configuration

## Project Structure

```
nixos-easy-install/
├── installer/           # Rust Windows application
├── initrd/              # Unattended NixOS installer
├── bootloader/          # Signed EFI components
├── configs/
│   ├── starter/         # Beginner-friendly config
│   └── minimal/         # Bare minimum config
└── docs/
```

## Building

Requires NixOS or Nix with flakes enabled.

```bash
# Build Windows installer
nix build .#installer

# Build installer initrd
nix build .#initrd

# Build everything
nix build
```

## Development Status

🚧 **Early Development** - Not yet functional

## License

GPL-3.0 (to be compatible with NixOS and wubiuefi components)
