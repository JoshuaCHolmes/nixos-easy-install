# Installer for Snapdragon X Elite devices (Lenovo Yoga Slim 7x, ThinkPad T14s Gen 6, etc.)
# 
# This uses the x1e-nixos-config kernel and modules which have proper support
# for Qualcomm Snapdragon X Elite SoCs.

{ pkgs, x1e-nixos-config }:

let
  # Full installer script - mirrors default.nix but with X1E-specific handling
  installerScript = pkgs.writeShellScriptBin "nixos-easy-installer" ''
    set -euo pipefail
    
    export PATH="${pkgs.lib.makeBinPath (with pkgs; [
      coreutils util-linux e2fsprogs dosfstools parted
      nix git curl jq ntfs3g kmod gawk pciutils dmidecode usbutils
    ])}:$PATH"

    CONFIG_PATH="/boot/efi/EFI/NixOS/install-config.json"
    LOG="/tmp/install.log"
    
    log() {
      echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG"
    }
    
    fail() {
      log "FATAL: $*"
      log "Installation failed. Dropping to shell for debugging."
      log "Check $LOG for details."
      exec /bin/bash
    }
    
    # ============================================================
    # Initial Setup
    # ============================================================
    
    clear
    echo ""
    echo "  ███╗   ██╗██╗██╗  ██╗ ██████╗ ███████╗"
    echo "  ████╗  ██║██║╚██╗██╔╝██╔═══██╗██╔════╝"
    echo "  ██╔██╗ ██║██║ ╚███╔╝ ██║   ██║███████╗"
    echo "  ██║╚██╗██║██║ ██╔██╗ ██║   ██║╚════██║"
    echo "  ██║ ╚████║██║██╔╝ ╚██╗╚██████╔╝███████║"
    echo "  ╚═╝  ╚═══╝╚═╝╚═╝   ╚═╝ ╚═════╝ ╚══════╝"
    echo ""
    echo "    Easy Install - Snapdragon X Elite"
    echo ""
    
    log "NixOS Easy Install (X1E) - Starting"
    log "===================================="
    log "Hardware: Snapdragon X Elite detected"
    
    # Mount ESP to find config
    log "Looking for EFI System Partition..."
    mkdir -p /boot/efi
    
    # Find ESP by partition type GUID
    ESP_DEV=$(lsblk -rno NAME,PARTTYPE | grep -i 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b' | head -1 | cut -d' ' -f1)
    if [[ -z "$ESP_DEV" ]]; then
      fail "Could not find EFI System Partition"
    fi
    
    mount "/dev/$ESP_DEV" /boot/efi || fail "Could not mount ESP"
    log "ESP mounted: /dev/$ESP_DEV -> /boot/efi"
    
    # ============================================================
    # Read configuration
    # ============================================================
    
    if [[ ! -f "$CONFIG_PATH" ]]; then
      fail "Config not found at $CONFIG_PATH"
    fi
    
    CONFIG=$(cat "$CONFIG_PATH")
    log "Configuration loaded from $CONFIG_PATH"
    
    INSTALL_TYPE=$(echo "$CONFIG" | jq -r '.install_type')
    HOSTNAME=$(echo "$CONFIG" | jq -r '.hostname')
    USERNAME=$(echo "$CONFIG" | jq -r '.username')
    PASSWORD_HASH=$(echo "$CONFIG" | jq -r '.password_hash')
    FLAKE_TYPE=$(echo "$CONFIG" | jq -r '.flake.type')
    FLAKE_URL=$(echo "$CONFIG" | jq -r '.flake.url // empty')
    FLAKE_HOSTNAME=$(echo "$CONFIG" | jq -r '.flake.hostname // .hostname')
    
    log "Install type: $INSTALL_TYPE"
    log "Hostname: $HOSTNAME"
    log "Username: $USERNAME"
    log "Flake: $FLAKE_TYPE''${FLAKE_URL:+ ($FLAKE_URL)}"
    
    # ============================================================
    # Partition/Mount setup (loopback only for X1E currently)
    # ============================================================
    
    if [[ "$INSTALL_TYPE" == "loopback" || "$INSTALL_TYPE" == "quick" ]]; then
      log "Setting up loopback installation..."
      
      TARGET_DIR=$(echo "$CONFIG" | jq -r '.loopback.target_dir')
      SIZE_GB=$(echo "$CONFIG" | jq -r '.loopback.size_gb')
      
      log "Target directory: $TARGET_DIR"
      log "Size: ''${SIZE_GB}GB"
      
      # Find the correct Windows NTFS partition
      log "Looking for Windows partitions..."
      WINDOWS_PART=""
      
      NTFS_PARTS=$(lsblk -rno NAME,FSTYPE | grep -E 'ntfs|ntfs3' | cut -d' ' -f1)
      
      if [[ -z "$NTFS_PARTS" ]]; then
        fail "No NTFS partitions found"
      fi
      
      log "Found NTFS partitions: $NTFS_PARTS"
      
      # Try each NTFS partition to find the one with our NixOS directory
      for PART in $NTFS_PARTS; do
        log "Checking /dev/$PART..."
        mkdir -p /mnt/check
        
        if mount -t ntfs3 -o ro "/dev/$PART" /mnt/check 2>/dev/null; then
          # Convert target path for checking (C:\NixOS -> NixOS)
          CHECK_PATH="/mnt/check/$(echo "$TARGET_DIR" | sed 's|^[A-Za-z]:\\||; s|\\|/|g')"
          
          if [[ -d "$CHECK_PATH" ]]; then
            log "Found NixOS directory on /dev/$PART"
            WINDOWS_PART="$PART"
            umount /mnt/check
            break
          fi
          umount /mnt/check
        fi
      done
      
      # Fallback to largest NTFS partition
      if [[ -z "$WINDOWS_PART" ]]; then
        log "NixOS directory not found, using largest NTFS partition..."
        WINDOWS_PART=$(lsblk -rno NAME,FSTYPE,SIZE | grep -E 'ntfs|ntfs3' | sort -t' ' -k3 -h | tail -1 | cut -d' ' -f1)
        log "Selected partition: $WINDOWS_PART"
      fi
      
      if [[ -z "$WINDOWS_PART" ]]; then
        fail "Could not find suitable Windows NTFS partition"
      fi
      
      mkdir -p /mnt/windows
      mount -t ntfs3 "/dev/$WINDOWS_PART" /mnt/windows || fail "Could not mount Windows partition /dev/$WINDOWS_PART"
      log "Windows partition mounted: /dev/$WINDOWS_PART"
      
      # Derive NixOS directory from target_dir
      NIXOS_DIR="/mnt/windows/$(echo "$TARGET_DIR" | sed 's|^[A-Za-z]:\\||; s|\\|/|g')"
      log "NixOS directory: $NIXOS_DIR"
      
      if [[ ! -d "$NIXOS_DIR" ]]; then
        fail "NixOS directory not found: $NIXOS_DIR"
      fi
      
      # Create root.disk if needed
      if [[ ! -f "$NIXOS_DIR/root.disk" ]]; then
        log "Creating root.disk (''${SIZE_GB}GB)..."
        truncate -s "''${SIZE_GB}G" "$NIXOS_DIR/root.disk"
      fi
      
      # Format if needed
      if ! file "$NIXOS_DIR/root.disk" | grep -q 'ext4'; then
        log "Formatting root.disk as ext4..."
        mkfs.ext4 -F -L NIXOS_ROOT "$NIXOS_DIR/root.disk"
      fi
      
      # Mount loopback
      log "Mounting root.disk..."
      mkdir -p /mnt
      mount -o loop "$NIXOS_DIR/root.disk" /mnt || fail "Could not mount root.disk"
      
      # Bind mount ESP
      mkdir -p /mnt/boot
      mount --bind /boot/efi /mnt/boot
      
      log "Loopback filesystems mounted"
    else
      fail "X1E installer currently only supports loopback installation"
    fi
    
    # ============================================================
    # Generate hardware configuration
    # ============================================================
    
    log "Generating hardware configuration..."
    mkdir -p /mnt/etc/nixos
    mkdir -p /mnt/etc/nixos-generated
    nixos-generate-config --root /mnt --dir /mnt/etc/nixos-generated
    cp /mnt/etc/nixos-generated/*.nix /mnt/etc/nixos/
    
    # Add loopback + X1E specific config
    log "Adding X1E and loopback-specific configuration..."
    
    for hwconf in /mnt/etc/nixos-generated/hardware-configuration.nix /mnt/etc/nixos/hardware-configuration.nix; do
      if [[ -f "$hwconf" ]]; then
        cat >> "$hwconf" << 'EOF'

  # Loopback installation - boot from disk image on NTFS
  boot.initrd.supportedFilesystems = [ "ntfs3" ];
  boot.initrd.availableKernelModules = [ "loop" "ntfs3" ];
  
  # X1E-specific kernel parameters (critical for display/USB init)
  boot.kernelParams = [ "pd_ignore_unused" "clk_ignore_unused" ];
EOF
      fi
    done
    
    # ============================================================
    # Setup flake configuration
    # ============================================================
    
    log "Setting up NixOS configuration..."
    FLAKE_DIR="/mnt/etc/nixos"
    
    if [[ "$FLAKE_TYPE" == "url" && -n "$FLAKE_URL" ]]; then
      log "Cloning configuration from $FLAKE_URL..."
      rm -rf "$FLAKE_DIR"
      git clone "$FLAKE_URL" "$FLAKE_DIR" || fail "Could not clone flake from $FLAKE_URL"
      
    elif [[ "$FLAKE_TYPE" == "starter" || "$FLAKE_TYPE" == "minimal" ]]; then
      log "Creating X1E starter configuration..."
      
      cat > "$FLAKE_DIR/flake.nix" << EOF
{
  description = "NixOS configuration for Snapdragon X Elite";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    x1e-nixos-config = {
      url = "github:kuruczgy/x1e-nixos-config";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, x1e-nixos-config, ... }: {
    nixosConfigurations.$HOSTNAME = nixpkgs.lib.nixosSystem {
      system = "aarch64-linux";
      modules = [
        x1e-nixos-config.nixosModules.x1e
        ./configuration.nix
        ./hardware-configuration.nix
      ];
    };
  };
}
EOF

      cat > "$FLAKE_DIR/configuration.nix" << EOF
{ config, pkgs, ... }:

{
  system.stateVersion = "24.11";
  
  # Enable Lenovo Yoga Slim 7x hardware support
  # Change this for other X1E devices (e.g., hardware.thinkpad-t14s-gen6.enable)
  hardware.lenovo-yoga-slim7x.enable = true;
  
  # Network
  networking.hostName = "$HOSTNAME";
  networking.networkmanager.enable = true;
  
  # Timezone (change as needed)
  time.timeZone = "America/Chicago";
  
  # User account
  users.users.$USERNAME = {
    isNormalUser = true;
    extraGroups = [ "wheel" "networkmanager" "video" "audio" ];
    hashedPassword = "$PASSWORD_HASH";
  };
  
  # Essential packages
  environment.systemPackages = with pkgs; [
    vim git curl wget htop
  ];
  
  # Enable sudo
  security.sudo.enable = true;
  
  # Boot configuration for loopback install
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
}
EOF

      log "Starter configuration created"
    fi
    
    # ============================================================
    # Run nixos-install
    # ============================================================
    
    log "Starting nixos-install (this may take a while)..."
    log "You can follow progress in another tty (Alt+F2)"
    
    INSTALL_HOSTNAME="$HOSTNAME"
    
    if [[ -f "$FLAKE_DIR/flake.nix" ]]; then
      log "Installing from flake: $FLAKE_DIR#$INSTALL_HOSTNAME"
      nixos-install --root /mnt \
        --flake "$FLAKE_DIR#$INSTALL_HOSTNAME" \
        --no-root-passwd \
        --no-channel-copy \
        2>&1 | tee -a "$LOG"
    else
      log "Installing from configuration.nix"
      nixos-install --root /mnt \
        --no-root-passwd \
        2>&1 | tee -a "$LOG"
    fi
    
    INSTALL_EXIT=$?
    if [[ $INSTALL_EXIT -ne 0 ]]; then
      fail "nixos-install failed with exit code $INSTALL_EXIT"
    fi
    
    log "NixOS installation complete!"
    
    # ============================================================
    # Post-install cleanup
    # ============================================================
    
    log "Performing post-install cleanup..."
    
    # Remove install config (contains password hash)
    rm -f "$CONFIG_PATH"
    
    # Copy install log
    cp "$LOG" /mnt/var/log/nixos-easy-install.log 2>/dev/null || true
    
    # ============================================================
    # Done!
    # ============================================================
    
    log ""
    log "============================================"
    log "   Installation Complete!"
    log "============================================"
    log ""
    log "Your NixOS system has been installed to the loopback disk."
    log ""
    log "Next steps:"
    log "  1. Reboot into Windows"
    log "  2. Select NixOS from the boot menu"
    log "  3. Log in as '$USERNAME'"
    log ""
    log "IMPORTANT: Your configuration uses x1e-nixos-config for"
    log "Snapdragon X Elite support. Keep it updated!"
    log ""
    log "Press Enter to reboot, or Ctrl+C to drop to shell..."
    read -r
    
    umount -R /mnt 2>/dev/null || true
    reboot
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
