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
      HOSTNAME=$(jq -r '.hostname' "$CONFIG_FILE")
      USERNAME=$(jq -r '.username' "$CONFIG_FILE")
      # Flake config - get URL from flake.url or flake config
      FLAKE_TYPE=$(jq -r '.flake.type // "starter"' "$CONFIG_FILE")
      CONFIG_URL=$(jq -r '.flake.url // empty' "$CONFIG_FILE")
      FLAKE_HOSTNAME=$(jq -r '.flake.hostname // .hostname' "$CONFIG_FILE")
      # Loopback config
      LOOPBACK_TARGET=$(jq -r '.loopback.target_dir // empty' "$CONFIG_FILE")
      LOOPBACK_SIZE=$(jq -r '.loopback.size_gb // 32' "$CONFIG_FILE")
      
      echo "Install type: $INSTALL_TYPE"
      echo "Hostname: $HOSTNAME"
      echo "Flake type: $FLAKE_TYPE"
      [ -n "$CONFIG_URL" ] && echo "Config URL: $CONFIG_URL"
      
      if [ "$INSTALL_TYPE" = "loopback" ] && [ -n "$LOOPBACK_TARGET" ]; then
        echo "Loopback target: $LOOPBACK_TARGET"
        
        # Mount the NTFS partition and setup the loopback
        # Note: Path like C:\NixOS or C:/NixOS means the C: partition
        # Convert backslashes to forward slashes and extract drive letter
        LOOPBACK_PATH=$(echo "$LOOPBACK_TARGET" | sed 's|\\|/|g')
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
      
      # X1E-specific kernel parameters - prevent clock/power domain shutdown
      # before drivers load (display, USB, etc. won't initialize without these)
      # These are also passed via GRUB, but having them here ensures they're
      # available if the system is booted via other means
      boot.kernelParams = [
        "pd_ignore_unused"
        "clk_ignore_unused"
      ];
      
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
    nativeBuildInputs = [ pkgs.coreutils pkgs.findutils ];
  } ''
    mkdir -p $out
    
    # Copy kernel (ARM64 uses Image, not bzImage typically)
    cp ${nixosSystem.config.system.build.kernel}/*Image $out/bzImage 2>/dev/null || \
      cp ${nixosSystem.config.system.build.kernel}/Image $out/bzImage 2>/dev/null || \
      cp ${nixosSystem.config.system.build.kernel}/bzImage $out/bzImage
    
    cp ${nixosSystem.config.system.build.initialRamdisk}/initrd $out/initrd
    
    # Copy Device Tree Blob - REQUIRED for X1E hardware initialization
    # The DTB tells the kernel about the specific hardware (display, USB, etc.)
    DTB_PKG="${nixosSystem.config.hardware.deviceTree.package or ""}"
    DTB_NAME="${nixosSystem.config.hardware.deviceTree.name or ""}"
    
    mkdir -p $out/dtbs
    
    # Try to find and copy DTBs from the deviceTree package
    if [ -n "$DTB_PKG" ] && [ -d "$DTB_PKG/dtbs" ]; then
      cp -r "$DTB_PKG/dtbs/"* $out/dtbs/ 2>/dev/null || true
    fi
    
    # Also try the kernel's built-in DTBs
    KERNEL_DTBS="${nixosSystem.config.system.build.kernel}/dtbs"
    if [ -d "$KERNEL_DTBS" ]; then
      cp -r "$KERNEL_DTBS/"* $out/dtbs/ 2>/dev/null || true
    fi
    
    # Copy the specific DTB we need (Yoga Slim 7x)
    YOGA_DTB="qcom/x1e80100-lenovo-yoga-slim7x.dtb"
    if [ -f "$out/dtbs/$YOGA_DTB" ]; then
      cp "$out/dtbs/$YOGA_DTB" $out/device.dtb
      echo "$YOGA_DTB" > $out/dtb-name
    elif [ -n "$DTB_NAME" ] && [ -f "$out/dtbs/$DTB_NAME" ]; then
      cp "$out/dtbs/$DTB_NAME" $out/device.dtb
      echo "$DTB_NAME" > $out/dtb-name
    else
      echo "WARNING: Could not find DTB - checking all available:" >&2
      find $out/dtbs -name "*.dtb" -type f 2>/dev/null | head -10 >&2 || true
      
      # Try to find any X1E DTB as fallback
      X1E_DTB=$(find $out/dtbs -name "*x1e80100*.dtb" -type f 2>/dev/null | head -1 || true)
      if [ -n "$X1E_DTB" ]; then
        echo "Found fallback X1E DTB: $X1E_DTB" >&2
        cp "$X1E_DTB" $out/device.dtb
        basename "$X1E_DTB" > $out/dtb-name
      fi
    fi
    
    # Verify critical files exist
    if [ ! -f "$out/bzImage" ]; then
      echo "ERROR: Kernel image not found!" >&2
      exit 1
    fi
    if [ ! -f "$out/initrd" ]; then
      echo "ERROR: Initrd not found!" >&2
      exit 1
    fi
    if [ ! -f "$out/device.dtb" ]; then
      echo "ERROR: Device Tree Blob not found - X1E will not boot correctly!" >&2
      echo "DTB_PKG was: $DTB_PKG" >&2
      echo "DTB_NAME was: $DTB_NAME" >&2
      ls -la $out/dtbs/ 2>/dev/null || echo "No dtbs directory" >&2
      exit 1
    fi
    
    # Export the init path - required for booting NixOS
    echo "${nixosSystem.config.system.build.toplevel}/init" > $out/init-path
    
    # Export device info for the Windows installer
    # Must match the arch string expected by installer (assets.rs detect_platform)
    echo "aarch64-x1e" > $out/platform
    echo "lenovo-yoga-slim7x" > $out/default-device
    
    cd $out
    sha256sum bzImage initrd init-path platform default-device device.dtb > SHA256SUMS
  '';
  
  # Default is the toplevel for backwards compatibility
  default = nixosSystem.config.system.build.toplevel;
}
