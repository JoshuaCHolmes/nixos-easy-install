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
        
        # Customize the starter config with user's settings
        log "Customizing starter configuration..."
        
        # Detect system architecture
        ARCH=$(uname -m)
        if [[ "$ARCH" == "aarch64" ]]; then
          SYSTEM="aarch64-linux"
        else
          SYSTEM="x86_64-linux"
        fi
        
        # Rename hosts/default to hosts/<hostname> for proper multi-machine support
        if [[ -d "$FLAKE_DIR/hosts/default" ]]; then
          mv "$FLAKE_DIR/hosts/default" "$FLAKE_DIR/hosts/$HOSTNAME"
          log "Renamed hosts/default to hosts/$HOSTNAME"
        fi
        
        # Rewrite flake.nix to use hostname-based configuration
        cat > "$FLAKE_DIR/flake.nix" << FLAKE
{
  description = "My NixOS Configuration";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, home-manager, ... }: 
  let
    mkHost = { 
      system, 
      hostname, 
      username ? "$USERNAME",
      extraModules ? [],
    }: nixpkgs.lib.nixosSystem {
      inherit system;
      specialArgs = { inherit self username; };
      modules = [
        home-manager.nixosModules.home-manager
        ./modules/common.nix
        {
          networking.hostName = hostname;
          home-manager.users.\''${username} = import ./home;
          home-manager.extraSpecialArgs = { inherit username; };
        }
      ] ++ extraModules;
    };
  in
  {
    nixosConfigurations = {
      # $HOSTNAME - installed $(date +%Y-%m-%d)
      "$HOSTNAME" = mkHost {
        system = "$SYSTEM";
        hostname = "$HOSTNAME";
        username = "$USERNAME";
        extraModules = [
          ./hosts/$HOSTNAME
        ];
      };

      # To add another machine:
      # 1. Create hosts/<new-hostname>/default.nix
      # 2. Copy hardware-configuration.nix from the new machine
      # 3. Add a new entry here following the pattern above
    };
  };
}
FLAKE
        
        # Add user password to common.nix
        if [[ -f "$FLAKE_DIR/modules/common.nix" ]]; then
          sed -i "/isNormalUser = true;/a\\    hashedPassword = \"$PASSWORD_HASH\";" "$FLAKE_DIR/modules/common.nix"
        fi
        
        # Remove upstream git history - this is now the user's config
        rm -rf "$FLAKE_DIR/.git"
        git -C "$FLAKE_DIR" init
        git -C "$FLAKE_DIR" add -A
        git -C "$FLAKE_DIR" commit -m "Initial NixOS configuration for $HOSTNAME"
        
        log "Starter config customized: hostname=$HOSTNAME, user=$USERNAME"
        log "Config initialized as fresh git repo (no upstream tracking)"
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
        
        # Clone the config
        ORIG_CONFIG="/mnt/etc/nixos-original"
        rm -rf "$ORIG_CONFIG"
        git clone "$FLAKE_URL" "$ORIG_CONFIG" || fail "Could not clone $FLAKE_URL"
        
        # Check if this config already has a host definition for this hostname
        if grep -q "\"$HOSTNAME\"" "$ORIG_CONFIG/flake.nix" 2>/dev/null; then
          log "Found existing host definition for $HOSTNAME"
          rm -rf "$FLAKE_DIR"
          mv "$ORIG_CONFIG" "$FLAKE_DIR"
          URL_NEEDS_WRAPPER=false
          URL_NEEDS_NEW_HOST=false
          
        # Check if config has hosts/ structure (multi-machine config)
        elif [[ -d "$ORIG_CONFIG/hosts" ]] && grep -rq "hardware-configuration" "$ORIG_CONFIG"/**/*.nix 2>/dev/null; then
          log "Detected multi-machine config with hosts/ structure"
          log "Adding new host: $HOSTNAME"
          
          rm -rf "$FLAKE_DIR"
          mv "$ORIG_CONFIG" "$FLAKE_DIR"
          URL_NEEDS_WRAPPER=false
          URL_NEEDS_NEW_HOST=true
          
          # Create new host directory
          mkdir -p "$FLAKE_DIR/hosts/$HOSTNAME"
          
          # Copy template from existing host or create minimal one
          EXISTING_HOST=$(ls -1 "$FLAKE_DIR/hosts" | grep -v "wsl" | head -1)
          if [[ -n "$EXISTING_HOST" && -f "$FLAKE_DIR/hosts/$EXISTING_HOST/default.nix" ]]; then
            cp "$FLAKE_DIR/hosts/$EXISTING_HOST/default.nix" "$FLAKE_DIR/hosts/$HOSTNAME/default.nix"
            log "Copied host template from $EXISTING_HOST"
          else
            # Create minimal host config
            cat > "$FLAKE_DIR/hosts/$HOSTNAME/default.nix" << 'HOSTCONF'
{ config, lib, pkgs, ... }:

{
  imports = [
    ./hardware-configuration.nix
  ];

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  system.stateVersion = "24.11";
}
HOSTCONF
          fi
          
          # Hardware config will be copied in integration section
          
          # Add host entry to flake.nix
          # Find the pattern used and add a new entry
          ARCH=$(uname -m)
          if [[ "$ARCH" == "aarch64" ]]; then
            SYSTEM="aarch64-linux"
          else
            SYSTEM="x86_64-linux"
          fi
          
          # Try to add entry to flake.nix (before the closing brace of nixosConfigurations)
          # This is fragile but covers common patterns
          if grep -q "mkHost" "$FLAKE_DIR/flake.nix"; then
            # Uses mkHost pattern - add similar entry
            sed -i "/nixosConfigurations = {/a\\\\n      # $HOSTNAME - added $(date +%Y-%m-%d)\\n      \"$HOSTNAME\" = mkHost {\\n        system = \"$SYSTEM\";\\n        hostname = \"$HOSTNAME\";\\n        username = \"$USERNAME\";\\n        extraModules = [\\n          ./hosts/$HOSTNAME\\n        ];\\n      };" "$FLAKE_DIR/flake.nix"
          else
            log "WARNING: Could not auto-add host to flake.nix"
            log "         You may need to manually add $HOSTNAME to nixosConfigurations"
          fi
          
          # Commit the new host
          git -C "$FLAKE_DIR" add -A
          git -C "$FLAKE_DIR" commit -m "Add host: $HOSTNAME" || true
          
        # Config imports hardware-configuration.nix somewhere
        elif grep -rq "hardware-configuration" "$ORIG_CONFIG"/*.nix "$ORIG_CONFIG"/**/*.nix 2>/dev/null; then
          log "Config imports hardware-configuration.nix - using directly"
          rm -rf "$FLAKE_DIR"
          mv "$ORIG_CONFIG" "$FLAKE_DIR"
          URL_NEEDS_WRAPPER=false
          URL_NEEDS_NEW_HOST=false
          
        # WSL config (doesn't need hardware config)
        elif grep -rq "wsl.enable\|nixos-wsl" "$ORIG_CONFIG" 2>/dev/null; then
          log "Detected WSL configuration - using directly (no hardware config needed)"
          rm -rf "$FLAKE_DIR"
          mv "$ORIG_CONFIG" "$FLAKE_DIR"
          URL_NEEDS_WRAPPER=false
          URL_NEEDS_NEW_HOST=false
          
        # No hardware import - need wrapper
        else
          log "Config doesn't import hardware-configuration.nix"
          log "Creating wrapper flake to inject hardware support..."
          URL_NEEDS_WRAPPER=true
          URL_NEEDS_NEW_HOST=false
          
          # Create wrapper flake that uses extendModules
          rm -rf "$FLAKE_DIR"
          mkdir -p "$FLAKE_DIR"
          
          # Copy hardware config to wrapper
          cp /mnt/etc/nixos-generated/hardware-configuration.nix "$FLAKE_DIR/"
          
          cat > "$FLAKE_DIR/flake.nix" << 'WRAPPER'
{
  description = "NixOS Easy Install - Wrapper for custom configuration";
  
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    
    user-config = {
WRAPPER
          # Add the URL dynamically (outside of heredoc to avoid escaping issues)
          echo "      url = \"$FLAKE_URL\";" >> "$FLAKE_DIR/flake.nix"
          
          cat >> "$FLAKE_DIR/flake.nix" << 'WRAPPER'
    };
  };
  
  outputs = { self, nixpkgs, user-config, ... }:
  let
    # Find the target configuration in the user's flake
WRAPPER
          # Add hostname references
          echo "    targetHostname = \"$FLAKE_HOSTNAME\";" >> "$FLAKE_DIR/flake.nix"
          echo "    newHostname = \"$HOSTNAME\";" >> "$FLAKE_DIR/flake.nix"
          echo "    username = \"$USERNAME\";" >> "$FLAKE_DIR/flake.nix"
          echo "    passwordHash = \"$PASSWORD_HASH\";" >> "$FLAKE_DIR/flake.nix"
          
          cat >> "$FLAKE_DIR/flake.nix" << 'WRAPPER'
    
    # Get the original system - try exact hostname first, then first available
    originalSystem = 
      user-config.nixosConfigurations.''${targetHostname} or
      (builtins.head (builtins.attrValues user-config.nixosConfigurations));
    
    # Use extendModules to add hardware config to the original system
    extendedSystem = originalSystem.extendModules {
      modules = [
        ./hardware-configuration.nix
        
        # Override hostname
        { networking.hostName = nixpkgs.lib.mkForce newHostname; }
        
        # Ensure user exists with password
        ({ config, lib, ... }: {
          users.users.''${username} = lib.mkIf (!config.users.users ? username) {
            isNormalUser = true;
            extraGroups = [ "wheel" "networkmanager" "video" "audio" ];
            hashedPassword = passwordHash;
          };
          # Also set password for existing user if they exist but have no password set
          users.users.''${username}.hashedPassword = lib.mkDefault passwordHash;
        })
      ];
    };
  in {
    nixosConfigurations.''${newHostname} = extendedSystem;
  };
}
WRAPPER
          log "Wrapper flake created at $FLAKE_DIR"
          log "Original config preserved at $ORIG_CONFIG for reference"
        fi
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
    
    HWCONF_SRC="/mnt/etc/nixos-generated/hardware-configuration.nix"
    
    # Skip if URL type with wrapper (hardware config already included in wrapper)
    if [[ "$FLAKE_TYPE" == "url" && "''${URL_NEEDS_WRAPPER:-false}" == "true" ]]; then
      log "Hardware config already integrated via wrapper flake"
      
    # Skip for minimal (already has hardware-configuration.nix at root)
    elif [[ "$FLAKE_TYPE" == "minimal" ]]; then
      log "Hardware config already at root level (minimal config)"
      
    # For starter and url (direct use), we need to place hardware-configuration.nix
    elif [[ -f "$FLAKE_DIR/flake.nix" ]]; then
      log "Integrating hardware configuration..."
      
      # Check if config imports hardware-configuration.nix
      if grep -rq "hardware-configuration" "$FLAKE_DIR"/*.nix "$FLAKE_DIR"/**/*.nix 2>/dev/null; then
        log "Config imports hardware-configuration.nix"
        
        # Determine where to place it based on directory structure
        if [[ -d "$FLAKE_DIR/hosts/$FLAKE_HOSTNAME" ]]; then
          HWCONF_DEST="$FLAKE_DIR/hosts/$FLAKE_HOSTNAME/hardware-configuration.nix"
          log "Placing hardware config in hosts/$FLAKE_HOSTNAME/"
        elif [[ -d "$FLAKE_DIR/hosts/default" ]]; then
          # Starter config pattern - use default host
          HWCONF_DEST="$FLAKE_DIR/hosts/default/hardware-configuration.nix"
          log "Placing hardware config in hosts/default/"
        elif [[ -d "$FLAKE_DIR/hosts" ]]; then
          mkdir -p "$FLAKE_DIR/hosts/$FLAKE_HOSTNAME"
          HWCONF_DEST="$FLAKE_DIR/hosts/$FLAKE_HOSTNAME/hardware-configuration.nix"
          log "Creating hosts/$FLAKE_HOSTNAME/ for hardware config"
        else
          HWCONF_DEST="$FLAKE_DIR/hardware-configuration.nix"
          log "Placing hardware config at root level"
        fi
        
        cp "$HWCONF_SRC" "$HWCONF_DEST"
        
      else
        # Config doesn't import hardware-configuration.nix
        if grep -rq "wsl.enable\|nixos-wsl" "$FLAKE_DIR" 2>/dev/null; then
          log "Detected WSL configuration - hardware config not needed"
        else
          log "WARNING: Config doesn't import hardware-configuration.nix"
          log "         This may cause boot issues on real hardware"
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
    
    # Determine which hostname to use for the flake
    # If we created a wrapper, use $HOSTNAME; otherwise use $FLAKE_HOSTNAME
    if [[ "$FLAKE_TYPE" == "url" && "''${URL_NEEDS_WRAPPER:-false}" == "true" ]]; then
      INSTALL_HOSTNAME="$HOSTNAME"
    elif [[ "$FLAKE_TYPE" == "starter" ]]; then
      # Starter config uses 'default' as the configuration name
      INSTALL_HOSTNAME="default"
    else
      INSTALL_HOSTNAME="$FLAKE_HOSTNAME"
    fi
    
    # Build the flake if present, otherwise use traditional install
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
    
    # Remove install config from ESP (contains password hash)
    rm -f "$CONFIG_PATH"
    
    # Remove installer boot entry
    # The NixOS boot config will take over
    rm -rf /boot/efi/EFI/NixOS/shimx64.efi.installer 2>/dev/null || true
    
    # Copy install log to new system
    cp "$LOG" /mnt/var/log/nixos-easy-install.log 2>/dev/null || true
    
    # Create helper script for config graduation (starter config only)
    if [[ "$FLAKE_TYPE" == "starter" ]]; then
      mkdir -p /mnt/usr/local/bin
      cat > /mnt/usr/local/bin/nixos-config-publish << 'SCRIPT'
#!/usr/bin/env bash
# Publish your NixOS configuration to a git repository
# Usage: nixos-config-publish <github-repo-url>
#   e.g: nixos-config-publish git@github.com:username/my-nixos-config.git

set -euo pipefail

CONFIG_DIR="/etc/nixos"

if [[ ! -d "$CONFIG_DIR/.git" ]]; then
  echo "Error: $CONFIG_DIR is not a git repository"
  exit 1
fi

if [[ $# -lt 1 ]]; then
  echo "Usage: nixos-config-publish <github-repo-url>"
  echo ""
  echo "This will:"
  echo "  1. Add the URL as 'origin' remote"
  echo "  2. Push your configuration"
  echo ""
  echo "First, create an empty repository on GitHub, then run:"
  echo "  nixos-config-publish git@github.com:YOUR_USERNAME/YOUR_REPO.git"
  exit 1
fi

REPO_URL="$1"

cd "$CONFIG_DIR"

# Check if origin already exists
if git remote get-url origin &>/dev/null; then
  echo "Remote 'origin' already exists: $(git remote get-url origin)"
  read -p "Replace with $REPO_URL? [y/N] " -n 1 -r
  echo
  if [[ $REPLY =~ ^[Yy]$ ]]; then
    git remote set-url origin "$REPO_URL"
  else
    exit 1
  fi
else
  git remote add origin "$REPO_URL"
fi

echo "Pushing to $REPO_URL..."
git push -u origin main || git push -u origin master

echo ""
echo "✓ Configuration published!"
echo ""
echo "Your config is now at: $REPO_URL"
echo ""
echo "To install on another machine, use:"
echo "  NixOS Easy Install → Custom URL → $REPO_URL"
SCRIPT
      chmod +x /mnt/usr/local/bin/nixos-config-publish
      log "Created /usr/local/bin/nixos-config-publish helper script"
    fi
    
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
    if [[ "$FLAKE_TYPE" == "starter" ]]; then
      log "║   To backup your config to GitHub:                         ║"
      log "║   nixos-config-publish <your-repo-url>                     ║"
      log "║                                                            ║"
    fi
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
