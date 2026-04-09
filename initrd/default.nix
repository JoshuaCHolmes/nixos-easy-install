{ pkgs, lib, ... }:

# This builds a minimal NixOS system that acts as an installer
# It boots, reads install-config.json, and performs unattended installation

let
  # The installer script that runs on boot
  installerScript = pkgs.writeShellScript "nixos-easy-installer" ''
    set -euo pipefail
    
    export PATH="${lib.makeBinPath (with pkgs; [
      coreutils util-linux e2fsprogs dosfstools parted
      nixos-install-tools nix git curl jq
      ntfs3g  # For loopback installs
    ])}:$PATH"

    CONFIG_PATH="/boot/efi/nixos-install/install-config.json"
    LOG="/tmp/install.log"
    
    log() {
      echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG"
    }
    
    fail() {
      log "FATAL: $*"
      log "Installation failed. Dropping to shell for debugging."
      exec /bin/sh
    }
    
    # ============================================================
    # Read configuration
    # ============================================================
    
    log "NixOS Easy Install - Unattended Installer"
    log "=========================================="
    
    if [[ ! -f "$CONFIG_PATH" ]]; then
      fail "Config not found at $CONFIG_PATH"
    fi
    
    CONFIG=$(cat "$CONFIG_PATH")
    log "Configuration loaded"
    
    INSTALL_TYPE=$(echo "$CONFIG" | jq -r '.install_type')
    HOSTNAME=$(echo "$CONFIG" | jq -r '.hostname')
    USERNAME=$(echo "$CONFIG" | jq -r '.username')
    PASSWORD_HASH=$(echo "$CONFIG" | jq -r '.password_hash')
    FLAKE_TYPE=$(echo "$CONFIG" | jq -r '.flake.type')
    FLAKE_URL=$(echo "$CONFIG" | jq -r '.flake.url')
    FLAKE_HOSTNAME=$(echo "$CONFIG" | jq -r '.flake.hostname // .hostname')
    
    log "Install type: $INSTALL_TYPE"
    log "Hostname: $HOSTNAME"
    log "Username: $USERNAME"
    log "Flake: $FLAKE_TYPE ($FLAKE_URL)"
    
    # ============================================================
    # Partition/Mount setup
    # ============================================================
    
    if [[ "$INSTALL_TYPE" == "loopback" ]]; then
      log "Setting up loopback installation..."
      
      TARGET_DIR=$(echo "$CONFIG" | jq -r '.loopback.target_dir')
      SIZE_GB=$(echo "$CONFIG" | jq -r '.loopback.size_gb')
      
      # Mount the Windows partition
      # TODO: Detect Windows partition dynamically
      WINDOWS_PART=$(lsblk -rno NAME,FSTYPE | grep ntfs | head -1 | cut -d' ' -f1)
      mount -t ntfs3 "/dev/$WINDOWS_PART" /mnt/windows
      
      NIXOS_DIR="/mnt/windows/NixOS"
      mkdir -p "$NIXOS_DIR"
      
      # Create root.disk if it doesn't exist
      if [[ ! -f "$NIXOS_DIR/root.disk" ]]; then
        log "Creating root.disk (''${SIZE_GB}GB)..."
        truncate -s "''${SIZE_GB}G" "$NIXOS_DIR/root.disk"
        mkfs.ext4 -F "$NIXOS_DIR/root.disk"
      fi
      
      # Mount loopback
      mkdir -p /mnt
      mount -o loop "$NIXOS_DIR/root.disk" /mnt
      
      # Mount EFI partition
      ESP=$(lsblk -rno NAME,PARTTYPE | grep -i 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b' | head -1 | cut -d' ' -f1)
      mkdir -p /mnt/boot
      mount "/dev/$ESP" /mnt/boot
      
    elif [[ "$INSTALL_TYPE" == "partition" ]]; then
      log "Setting up partition installation..."
      
      ROOT_PART=$(echo "$CONFIG" | jq -r '.partition.root')
      BOOT_PART=$(echo "$CONFIG" | jq -r '.partition.boot')
      SWAP_PART=$(echo "$CONFIG" | jq -r '.partition.swap')
      
      # Format partitions
      log "Formatting $ROOT_PART as ext4..."
      mkfs.ext4 -F "$ROOT_PART"
      
      if [[ "$SWAP_PART" != "null" ]]; then
        log "Setting up swap on $SWAP_PART..."
        mkswap "$SWAP_PART"
        swapon "$SWAP_PART"
      fi
      
      # Mount
      mount "$ROOT_PART" /mnt
      mkdir -p /mnt/boot
      mount "$BOOT_PART" /mnt/boot
      
    else
      fail "Unknown install type: $INSTALL_TYPE"
    fi
    
    log "Filesystems mounted"
    
    # ============================================================
    # Generate hardware configuration
    # ============================================================
    
    log "Generating hardware configuration..."
    nixos-generate-config --root /mnt
    
    # ============================================================
    # Fetch/setup flake configuration
    # ============================================================
    
    FLAKE_DIR="/mnt/etc/nixos"
    mkdir -p "$FLAKE_DIR"
    
    case "$FLAKE_TYPE" in
      starter)
        log "Cloning starter configuration..."
        git clone https://github.com/TODO/nixos-starter-config "$FLAKE_DIR"
        ;;
      minimal)
        log "Using minimal configuration..."
        # Generate a minimal flake.nix
        cat > "$FLAKE_DIR/flake.nix" << 'FLAKE'
    {
      inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
      
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
      boot.loader.systemd-boot.enable = true;
      boot.loader.efi.canTouchEfiVariables = true;
      
      networking.hostName = "$HOSTNAME";
      networking.networkmanager.enable = true;
      
      time.timeZone = "America/Chicago";
      
      users.users.$USERNAME = {
        isNormalUser = true;
        extraGroups = [ "wheel" "networkmanager" ];
        hashedPassword = "$PASSWORD_HASH";
      };
      
      environment.systemPackages = with pkgs; [ vim git ];
      
      system.stateVersion = "24.11";
    }
    CONF
        ;;
      url)
        log "Cloning configuration from $FLAKE_URL..."
        git clone "$FLAKE_URL" "$FLAKE_DIR"
        ;;
      local)
        log "Using local configuration (should already be in place)"
        ;;
      *)
        fail "Unknown flake type: $FLAKE_TYPE"
        ;;
    esac
    
    # ============================================================
    # Run nixos-install
    # ============================================================
    
    log "Running nixos-install..."
    
    # If the config is a flake, use --flake
    if [[ -f "$FLAKE_DIR/flake.nix" ]]; then
      nixos-install --root /mnt --flake "$FLAKE_DIR#$FLAKE_HOSTNAME" --no-root-passwd
    else
      nixos-install --root /mnt --no-root-passwd
    fi
    
    log "Installation complete!"
    
    # ============================================================
    # Cleanup and reboot
    # ============================================================
    
    log "Cleaning up installer files..."
    rm -f "$CONFIG_PATH"
    # TODO: Remove installer boot entry
    
    log "Installation successful! Rebooting in 5 seconds..."
    sleep 5
    reboot
  '';

in pkgs.stdenv.mkDerivation {
  pname = "nixos-easy-install-initrd";
  version = "0.1.0";
  
  src = ./.;
  
  # This is a placeholder - actual initrd building is more complex
  # and will use the NixOS module system
  buildPhase = ''
    mkdir -p $out
    cp ${installerScript} $out/installer.sh
  '';
  
  installPhase = ''
    echo "Initrd build placeholder"
  '';
  
  meta = with lib; {
    description = "Unattended NixOS installer initrd";
    license = licenses.gpl3;
  };
}
