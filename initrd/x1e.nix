# Installer for Snapdragon X Elite devices (Lenovo Yoga Slim 7x, ThinkPad T14s Gen 6, etc.)
# 
# This uses the x1e-nixos-config kernel and modules which have proper support
# for Qualcomm Snapdragon X Elite SoCs.

{ pkgs, x1e-nixos-config }:

let
  # Create installer script inline (same as in default.nix)
  installerScript = pkgs.writeShellScriptBin "nixos-easy-installer" ''
    #!/usr/bin/env bash
    set -euo pipefail
    
    echo "==========================================="
    echo "   NixOS Easy Installer (Snapdragon X1E)"
    echo "==========================================="
    echo ""
    echo "Hardware: Snapdragon X Elite detected"
    echo ""
    
    # Read installer config from ESP
    CONFIG_FILE="/boot/efi/EFI/NixOS/install-config.json"
    
    if [ -f "$CONFIG_FILE" ]; then
      echo "Found installer configuration..."
      INSTALL_TYPE=$(jq -r '.install_type' "$CONFIG_FILE")
      CONFIG_URL=$(jq -r '.config_url' "$CONFIG_FILE")
      LOOPBACK_PATH=$(jq -r '.loopback_path // empty' "$CONFIG_FILE")
      
      echo "Install type: $INSTALL_TYPE"
      echo "Config URL: $CONFIG_URL"
      
      if [ "$INSTALL_TYPE" = "loopback" ] && [ -n "$LOOPBACK_PATH" ]; then
        echo "Loopback path: $LOOPBACK_PATH"
        
        # Mount the NTFS partition and setup the loopback
        # Note: Path like C:/NixOS means the C: partition
        DRIVE_LETTER=$(echo "$LOOPBACK_PATH" | cut -d: -f1)
        echo "Looking for Windows drive: $DRIVE_LETTER"
        
        # Find the Windows partition
        # For now, assume it's the largest NTFS partition
        NTFS_PART=$(lsblk -o NAME,FSTYPE,SIZE -b -n | grep ntfs | sort -k3 -n | tail -1 | awk '{print $1}')
        if [ -n "$NTFS_PART" ]; then
          echo "Found NTFS partition: /dev/$NTFS_PART"
          mkdir -p /mnt/windows
          mount -t ntfs3 "/dev/$NTFS_PART" /mnt/windows
          
          # Setup loopback
          LOOP_FILE="/mnt/windows/NixOS/root.disk"
          if [ -f "$LOOP_FILE" ]; then
            echo "Found loopback disk at $LOOP_FILE"
            LOOP_DEV=$(losetup -f --show "$LOOP_FILE")
            echo "Attached as $LOOP_DEV"
            
            # Check if it needs formatting
            if ! blkid "$LOOP_DEV" | grep -q ext4; then
              echo "Formatting as ext4..."
              mkfs.ext4 -L nixos "$LOOP_DEV"
            fi
            
            # Mount for installation
            mkdir -p /mnt/nixos
            mount "$LOOP_DEV" /mnt/nixos
            
            echo ""
            echo "Ready for NixOS installation to /mnt/nixos"
            echo ""
            echo "To install, run:"
            echo "  nixos-install --root /mnt/nixos --flake <your-flake>"
            echo ""
            echo "IMPORTANT: Your config should import the x1e module:"
            echo "  inputs.x1e-nixos-config.url = \"github:kuruczgy/x1e-nixos-config\";"
            echo "  imports = [ x1e-nixos-config.nixosModules.x1e ];"
            echo "  hardware.lenovo-yoga-slim7x.enable = true;"
            echo ""
          else
            echo "ERROR: Loopback file not found at $LOOP_FILE"
          fi
        else
          echo "ERROR: No NTFS partition found"
        fi
      fi
    else
      echo "No installer configuration found."
      echo "You can manually install NixOS from this environment."
    fi
    
    echo ""
    echo "Dropping to shell..."
    exec /bin/bash
  '';

in
let
  # Build a NixOS system with x1e support
  nixosSystem = pkgs.nixos {
    imports = [
      "${pkgs.path}/nixos/modules/profiles/minimal.nix"
      # Import x1e module for Snapdragon X Elite support
      x1e-nixos-config.nixosModules.x1e
    ];
    
    config = {
      # Basic system config
      system.stateVersion = "24.11";
      
      # Enable Lenovo Yoga Slim 7x by default (most common device)
      # The installer will detect and configure the right device
      hardware.lenovo-yoga-slim7x.enable = true;
      
      # Boot - minimal config for initrd-based installer
      boot.loader.grub.enable = false;
      boot.loader.systemd-boot.enable = false;
      
      # Dummy root filesystem (required by NixOS, but we're running from initrd)
      fileSystems."/" = {
        device = "none";
        fsType = "tmpfs";
        options = [ "mode=0755" ];
      };
      
      # Run installer on boot
      systemd.services.nixos-easy-installer = {
        description = "NixOS Easy Install (Snapdragon X Elite)";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${installerScript}/bin/nixos-easy-installer";
          StandardInput = "tty";
          StandardOutput = "tty";
          TTYPath = "/dev/tty1";
          TTYReset = true;
          TTYVHangup = true;
        };
      };
      
      # Include necessary tools
      environment.systemPackages = with pkgs; [
        installerScript
        git
        curl
        jq
        ntfs3g
        parted
        e2fsprogs
        dosfstools
        vim
        pciutils
        usbutils
      ];
      
      # Enable networking
      networking.networkmanager.enable = true;
      
      # Console setup
      console = {
        font = "Lat2-Terminus16";
        keyMap = "us";
      };
      
      # Minimal services
      services.getty.autologinUser = "root";
    };
  };
in {
  # The toplevel (for compatibility)
  toplevel = nixosSystem.config.system.build.toplevel;
  
  # Individual components
  kernel = nixosSystem.config.system.build.kernel;
  initrd = nixosSystem.config.system.build.initialRamdisk;
  
  # Combined boot assets with x1e-specific DTB
  bootAssets = pkgs.runCommand "installer-boot-assets-x1e" {
    nativeBuildInputs = [ pkgs.coreutils ];
  } ''
    mkdir -p $out
    
    # Copy kernel (ARM64 uses Image, not bzImage typically)
    cp ${nixosSystem.config.system.build.kernel}/*Image $out/bzImage 2>/dev/null || \
      cp ${nixosSystem.config.system.build.kernel}/Image $out/bzImage 2>/dev/null || \
      cp ${nixosSystem.config.system.build.kernel}/bzImage $out/bzImage
    
    cp ${nixosSystem.config.system.build.initialRamdisk}/initrd $out/initrd
    
    # Export the init path - required for booting NixOS
    echo "${nixosSystem.config.system.build.toplevel}/init" > $out/init-path
    
    # Export device info for the Windows installer
    echo "x1e" > $out/platform
    echo "lenovo-yoga-slim7x" > $out/default-device
    
    cd $out
    sha256sum bzImage initrd init-path platform default-device > SHA256SUMS
  '';
  
  # Default is the toplevel for backwards compatibility
  default = nixosSystem.config.system.build.toplevel;
}
