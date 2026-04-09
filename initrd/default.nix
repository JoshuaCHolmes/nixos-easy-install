{ pkgs ? import <nixpkgs> {} }:

# This builds a minimal NixOS system that acts as an installer
# It boots, reads install-config.json from ESP, and performs unattended installation

let
  # Installer script that runs on boot
  installerScript = pkgs.writeShellScriptBin "nixos-easy-installer" ''
    set -euo pipefail
    
    export PATH="${pkgs.lib.makeBinPath (with pkgs; [
      coreutils util-linux e2fsprogs dosfstools parted
      nix git curl jq ntfs3g kmod gawk
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
    echo "        Easy Install - Unattended Installer"
    echo ""
    
    log "NixOS Easy Install - Starting"
    log "============================="
    
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
    # Partition/Mount setup
    # ============================================================
    
    if [[ "$INSTALL_TYPE" == "loopback" || "$INSTALL_TYPE" == "quick" ]]; then
      log "Setting up loopback installation..."
      
      TARGET_DIR=$(echo "$CONFIG" | jq -r '.loopback.target_dir')
      SIZE_GB=$(echo "$CONFIG" | jq -r '.loopback.size_gb')
      
      # Find and mount Windows NTFS partition
      log "Looking for Windows partition..."
      WINDOWS_PART=$(lsblk -rno NAME,FSTYPE | grep -E 'ntfs|ntfs3' | head -1 | cut -d' ' -f1)
      if [[ -z "$WINDOWS_PART" ]]; then
        fail "Could not find Windows NTFS partition"
      fi
      
      mkdir -p /mnt/windows
      mount -t ntfs3 "/dev/$WINDOWS_PART" /mnt/windows || fail "Could not mount Windows partition"
      log "Windows partition mounted: /dev/$WINDOWS_PART"
      
      # Derive NixOS directory from target_dir (convert Windows path)
      # C:\NixOS -> /mnt/windows/NixOS
      NIXOS_DIR="/mnt/windows/$(echo "$TARGET_DIR" | sed 's|^[A-Za-z]:\\||; s|\\|/|g')"
      log "NixOS directory: $NIXOS_DIR"
      
      if [[ ! -d "$NIXOS_DIR" ]]; then
        fail "NixOS directory not found: $NIXOS_DIR (Windows installer should have created this)"
      fi
      
      # Create root.disk if it doesn't exist (it should from Windows installer)
      if [[ ! -f "$NIXOS_DIR/root.disk" ]]; then
        log "Creating root.disk (''${SIZE_GB}GB)..."
        truncate -s "''${SIZE_GB}G" "$NIXOS_DIR/root.disk"
      fi
      
      # Format the disk image if needed
      if ! file "$NIXOS_DIR/root.disk" | grep -q 'ext4'; then
        log "Formatting root.disk as ext4..."
        mkfs.ext4 -F -L NIXOS_ROOT "$NIXOS_DIR/root.disk"
      fi
      
      # Mount loopback
      log "Mounting root.disk..."
      mkdir -p /mnt
      mount -o loop "$NIXOS_DIR/root.disk" /mnt || fail "Could not mount root.disk"
      
      # Bind mount ESP into the new root
      mkdir -p /mnt/boot
      mount --bind /boot/efi /mnt/boot
      
      log "Loopback filesystems mounted"
      
    elif [[ "$INSTALL_TYPE" == "partition" || "$INSTALL_TYPE" == "full" ]]; then
      log "Setting up partition installation..."
      
      ROOT_PART=$(echo "$CONFIG" | jq -r '.partition.root')
      BOOT_PART=$(echo "$CONFIG" | jq -r '.partition.boot')
      SWAP_PART=$(echo "$CONFIG" | jq -r '.partition.swap // empty')
      
      # Format root partition
      log "Formatting $ROOT_PART as ext4..."
      mkfs.ext4 -F -L NIXOS_ROOT "$ROOT_PART"
      
      if [[ -n "$SWAP_PART" ]]; then
        log "Setting up swap on $SWAP_PART..."
        mkswap -L NIXOS_SWAP "$SWAP_PART"
        swapon "$SWAP_PART"
      fi
      
      # Mount
      mount "$ROOT_PART" /mnt
      mkdir -p /mnt/boot
      mount "$BOOT_PART" /mnt/boot
      
      log "Partition filesystems mounted"
      
    else
      fail "Unknown install type: $INSTALL_TYPE"
    fi
    
    # ============================================================
    # Generate hardware configuration
    # ============================================================
    
    log "Generating hardware configuration..."
    mkdir -p /mnt/etc/nixos
    
    # Generate to a backup location first, so cloned configs can use it
    mkdir -p /mnt/etc/nixos-generated
    nixos-generate-config --root /mnt --dir /mnt/etc/nixos-generated
    
    # Copy to main location (will be overwritten by cloned configs if needed)
    cp /mnt/etc/nixos-generated/*.nix /mnt/etc/nixos/
    
    # For loopback installs, we need to add special boot config
    if [[ "$INSTALL_TYPE" == "loopback" || "$INSTALL_TYPE" == "quick" ]]; then
      log "Adding loopback-specific boot configuration..."
      
      # Append to both locations (generated backup and main)
      for hwconf in /mnt/etc/nixos-generated/hardware-configuration.nix /mnt/etc/nixos/hardware-configuration.nix; do
        if [[ -f "$hwconf" ]]; then
          cat >> "$hwconf" << 'EOF'

  # Loopback installation - boot from disk image on NTFS
  boot.initrd.supportedFilesystems = [ "ntfs3" ];
  boot.initrd.availableKernelModules = [ "loop" "ntfs3" ];
EOF
        fi
      done
    fi
    
    # ============================================================
    # Fetch/setup flake configuration
    # ============================================================
    
    FLAKE_DIR="/mnt/etc/nixos"
    
    case "$FLAKE_TYPE" in
      starter)
        log "Cloning starter configuration..."
        rm -rf "$FLAKE_DIR"
        git clone --depth 1 https://github.com/JoshuaCHolmes/nixos-starter-config "$FLAKE_DIR" || \
          fail "Could not clone starter config"
        # Hardware config will be handled in the integration section below
        ;;
        
      minimal)
        log "Creating minimal configuration..."
        
        cat > "$FLAKE_DIR/flake.nix" << 'FLAKE'
{
  description = "NixOS Easy Install - Minimal Configuration";
  
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  
  outputs = { self, nixpkgs }: {
    nixosConfigurations.nixos = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        ./hardware-configuration.nix
      ];
    };
  };
}
FLAKE

        cat > "$FLAKE_DIR/configuration.nix" << CONF
{ config, pkgs, ... }:

{
  # Boot loader
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
  
  # Networking
  networking.hostName = "$HOSTNAME";
  networking.networkmanager.enable = true;
  
  # Timezone (adjust as needed)
  time.timeZone = "America/Chicago";
  
  # User account
  users.users.$USERNAME = {
    isNormalUser = true;
    description = "$USERNAME";
    extraGroups = [ "wheel" "networkmanager" "video" "audio" ];
    hashedPassword = "$PASSWORD_HASH";
  };
  
  # Allow sudo without password for wheel group (convenience)
  security.sudo.wheelNeedsPassword = false;
  
  # Basic packages
  environment.systemPackages = with pkgs; [
    vim
    git
    curl
    htop
  ];
  
  # Enable SSH
  services.openssh.enable = true;
  
  # Firewall
  networking.firewall.enable = true;
  
  # NixOS version
  system.stateVersion = "24.11";
}
CONF
        ;;
        
      url)
        log "Cloning configuration from $FLAKE_URL..."
        rm -rf "$FLAKE_DIR"
        git clone --depth 1 "$FLAKE_URL" "$FLAKE_DIR" || fail "Could not clone $FLAKE_URL"
        ;;
        
      local)
        log "Using provided local configuration"
        ;;
        
      *)
        fail "Unknown flake type: $FLAKE_TYPE"
        ;;
    esac
    
    # ============================================================
    # Hardware configuration integration
    # ============================================================
    
    # For non-minimal configs, we need to ensure hardware-configuration.nix is imported
    if [[ "$FLAKE_TYPE" != "minimal" && -f "$FLAKE_DIR/flake.nix" ]]; then
      log "Checking hardware configuration integration..."
      
      HWCONF_SRC="/mnt/etc/nixos-generated/hardware-configuration.nix"
      
      # Try to find where the config expects hardware-configuration.nix
      # Common patterns:
      #   1. ./hardware-configuration.nix (root level)
      #   2. ./hosts/<hostname>/hardware-configuration.nix
      #   3. Not imported at all (WSL configs, etc.)
      
      # Check if flake.nix or any .nix file imports hardware-configuration.nix
      if grep -rq "hardware-configuration" "$FLAKE_DIR"/*.nix "$FLAKE_DIR"/**/*.nix 2>/dev/null; then
        log "Config imports hardware-configuration.nix"
        
        # Find where it's expected
        IMPORT_PATH=$(grep -rh "hardware-configuration" "$FLAKE_DIR" 2>/dev/null | head -1)
        
        # Check common locations
        if [[ -d "$FLAKE_DIR/hosts/$FLAKE_HOSTNAME" ]]; then
          # Host-specific directory exists, put it there
          HWCONF_DEST="$FLAKE_DIR/hosts/$FLAKE_HOSTNAME/hardware-configuration.nix"
          log "Placing hardware config in hosts/$FLAKE_HOSTNAME/"
        elif [[ -d "$FLAKE_DIR/hosts" ]]; then
          # Has hosts dir but not this hostname - create it
          mkdir -p "$FLAKE_DIR/hosts/$FLAKE_HOSTNAME"
          HWCONF_DEST="$FLAKE_DIR/hosts/$FLAKE_HOSTNAME/hardware-configuration.nix"
          log "Creating hosts/$FLAKE_HOSTNAME/ for hardware config"
        else
          # Put at root level
          HWCONF_DEST="$FLAKE_DIR/hardware-configuration.nix"
          log "Placing hardware config at root level"
        fi
        
        cp "$HWCONF_SRC" "$HWCONF_DEST"
        
      else
        log "Config does not import hardware-configuration.nix"
        log "This may be intentional (e.g., WSL configs)"
        
        # Check if this looks like a WSL config
        if grep -rq "wsl.enable\|nixos-wsl" "$FLAKE_DIR" 2>/dev/null; then
          log "Detected WSL configuration - hardware config not needed"
        else
          # Non-WSL config without hardware import - warn but continue
          log "WARNING: Config doesn't import hardware-configuration.nix"
          log "         This may cause boot issues on real hardware"
          log "         Consider adding: ./hardware-configuration.nix to your modules"
          
          # Still copy it in case they want to add it manually
          cp "$HWCONF_SRC" "$FLAKE_DIR/hardware-configuration.nix"
          log "Hardware config copied to $FLAKE_DIR/ for reference"
        fi
      fi
    fi
    
    # ============================================================
    # Run nixos-install
    # ============================================================
    
    log "Starting nixos-install (this may take a while)..."
    log "You can follow progress in another tty (Alt+F2)"
    
    # Build the flake if present, otherwise use traditional install
    if [[ -f "$FLAKE_DIR/flake.nix" ]]; then
      log "Installing from flake: $FLAKE_DIR#$FLAKE_HOSTNAME"
      nixos-install --root /mnt \
        --flake "$FLAKE_DIR#$FLAKE_HOSTNAME" \
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
    
    # Remove install config from ESP (contains password hash)
    rm -f "$CONFIG_PATH"
    
    # Remove installer boot entry
    # The NixOS boot config will take over
    rm -rf /boot/efi/EFI/NixOS/shimx64.efi.installer 2>/dev/null || true
    
    # Copy install log to new system
    cp "$LOG" /mnt/var/log/nixos-easy-install.log 2>/dev/null || true
    
    # ============================================================
    # Success!
    # ============================================================
    
    log ""
    log "╔════════════════════════════════════════════════════════════╗"
    log "║                                                            ║"
    log "║   Installation Complete!                                   ║"
    log "║                                                            ║"
    log "║   Your NixOS system is ready.                              ║"
    log "║   The system will reboot in 10 seconds.                    ║"
    log "║                                                            ║"
    log "║   Username: $USERNAME"
    log "║   Hostname: $HOSTNAME"
    log "║                                                            ║"
    log "╚════════════════════════════════════════════════════════════╝"
    log ""
    
    sleep 10
    umount -R /mnt 2>/dev/null || true
    reboot -f
  '';

in
# Build a minimal NixOS system for the installer
(pkgs.nixos {
  imports = [
    "${pkgs.path}/nixos/modules/profiles/minimal.nix"
    "${pkgs.path}/nixos/modules/profiles/all-hardware.nix"
  ];
  
  config = {
    # Basic system config
    system.stateVersion = "24.11";
    
    # Boot
    boot.loader.grub.enable = false;
    boot.loader.systemd-boot.enable = false;
    
    # Run installer on boot
    systemd.services.nixos-easy-installer = {
      description = "NixOS Easy Install";
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
}).config.system.build.toplevel
