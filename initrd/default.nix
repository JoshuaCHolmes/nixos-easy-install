{ pkgs ? import <nixpkgs> {} }:

# This builds a minimal NixOS system that acts as an installer
# It boots, reads install-config.json from ESP, and performs unattended installation

let
  # Installer script that runs on boot
  installerScript = pkgs.writeShellScriptBin "nixos-easy-installer" ''
    set -euo pipefail
    
    export PATH="${pkgs.lib.makeBinPath (with pkgs; [
      coreutils util-linux e2fsprogs dosfstools parted
      nix git curl jq ntfs3g kmod gawk pciutils dmidecode
    ])}:$PATH"

    CONFIG_PATH="/boot/efi/EFI/NixOS/install-config.json"
    LOG="/tmp/install.log"
    HARDWARE_REPORT="/tmp/hardware-detection.json"
    
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
      
      # Extract drive letter from target_dir (e.g., "C" from "C:\NixOS")
      TARGET_DRIVE=$(echo "$TARGET_DIR" | head -c 1 | tr '[:lower:]' '[:upper:]')
      log "Target drive letter: $TARGET_DRIVE"
      
      # Find the correct Windows NTFS partition
      # We need to find the partition that matches the drive letter from Windows
      # This is tricky because Linux doesn't know Windows drive letters directly
      # Strategy: Look for NTFS partitions and find one containing our NixOS folder
      
      log "Looking for Windows partitions..."
      WINDOWS_PART=""
      
      # List all NTFS partitions
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
          # Convert target path for checking
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
      
      # If we didn't find it by directory, fall back to largest NTFS partition
      # (usually the main Windows partition)
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
    # Hardware Detection and Auto-Configuration
    # ============================================================
    
    log "Detecting hardware for automatic configuration..."
    
    # Read DMI information
    SYS_VENDOR=$(cat /sys/devices/virtual/dmi/id/sys_vendor 2>/dev/null || echo "Unknown")
    PRODUCT_NAME=$(cat /sys/devices/virtual/dmi/id/product_name 2>/dev/null || echo "Unknown")
    PRODUCT_FAMILY=$(cat /sys/devices/virtual/dmi/id/product_family 2>/dev/null || echo "")
    CHASSIS_TYPE=$(cat /sys/devices/virtual/dmi/id/chassis_type 2>/dev/null || echo "")
    BOARD_NAME=$(cat /sys/devices/virtual/dmi/id/board_name 2>/dev/null || echo "")
    
    # Detect architecture
    ARCH=$(uname -m)
    IS_ARM=false
    if [[ "$ARCH" == "aarch64" ]]; then
      IS_ARM=true
    fi
    
    # Detect CPU information
    CPU_MODEL=$(grep -m1 "model name" /proc/cpuinfo 2>/dev/null | cut -d: -f2 | xargs || echo "Unknown")
    CPU_VENDOR=""
    if echo "$CPU_MODEL" | grep -qi "Intel"; then
      CPU_VENDOR="intel"
    elif echo "$CPU_MODEL" | grep -qi "AMD"; then
      CPU_VENDOR="amd"
    elif echo "$CPU_MODEL" | grep -qi "Qualcomm\|Oryon\|Snapdragon"; then
      CPU_VENDOR="qualcomm"
    elif echo "$CPU_MODEL" | grep -qi "Apple"; then
      CPU_VENDOR="apple"
    fi
    
    # Detect RAM size (for swap/hibernate configuration)
    RAM_KB=$(grep MemTotal /proc/meminfo | awk '{print $2}')
    RAM_MB=$((RAM_KB / 1024))
    RAM_GB=$((RAM_MB / 1024))
    # For hibernate, swap should be >= RAM. Add 2GB buffer for safety.
    SWAP_SIZE_MB=$((RAM_MB + 2048))
    log "Detected RAM: ''${RAM_GB}GB (''${RAM_MB}MB) - Recommended swap: ''${SWAP_SIZE_MB}MB"
    
    # Detect if laptop (chassis types 8, 9, 10, 11, 14 are laptops/portables)
    IS_LAPTOP=false
    case "$CHASSIS_TYPE" in
      8|9|10|11|14) IS_LAPTOP=true ;;
    esac
    # Also check for battery presence as fallback
    if [[ -d /sys/class/power_supply/BAT0 || -d /sys/class/power_supply/BAT1 ]]; then
      IS_LAPTOP=true
    fi
    
    # Detect GPU
    HAS_NVIDIA=false
    HAS_AMD_GPU=false
    HAS_INTEL_GPU=false
    NVIDIA_BUS_ID=""
    INTEL_BUS_ID=""
    AMD_BUS_ID=""
    NVIDIA_MODEL=""
    
    while IFS= read -r line; do
      if echo "$line" | grep -qi "nvidia"; then
        HAS_NVIDIA=true
        # Extract bus ID (e.g., "01:00.0" from "01:00.0 VGA compatible controller...")
        NVIDIA_BUS_ID=$(echo "$line" | grep -oP '^\S+' | sed 's/\.0$//')
        NVIDIA_MODEL=$(echo "$line" | sed 's/.*: //')
      elif echo "$line" | grep -qi "AMD.*Radeon\|AMD/ATI"; then
        HAS_AMD_GPU=true
        AMD_BUS_ID=$(echo "$line" | grep -oP '^\S+' | sed 's/\.0$//')
      elif echo "$line" | grep -qi "Intel.*Graphics\|Intel.*UHD\|Intel.*Iris"; then
        HAS_INTEL_GPU=true
        INTEL_BUS_ID=$(echo "$line" | grep -oP '^\S+' | sed 's/\.0$//')
      fi
    done < <(lspci 2>/dev/null | grep -i "vga\|3d\|display")
    
    # Detect Snapdragon / Qualcomm (ARM)
    IS_SNAPDRAGON=false
    SNAPDRAGON_MODEL=""
    if [[ "$IS_ARM" == "true" ]]; then
      if lspci 2>/dev/null | grep -qi "Qualcomm"; then
        IS_SNAPDRAGON=true
      fi
      # Check CPU info for Snapdragon
      if echo "$CPU_MODEL" | grep -qi "Qualcomm\|Oryon\|X1E\|X Elite"; then
        IS_SNAPDRAGON=true
        if echo "$CPU_MODEL" | grep -qi "X1E-78"; then
          SNAPDRAGON_MODEL="x1e78"
        elif echo "$CPU_MODEL" | grep -qi "X1E-80"; then
          SNAPDRAGON_MODEL="x1e80"
        elif echo "$CPU_MODEL" | grep -qi "X1E"; then
          SNAPDRAGON_MODEL="x1e"
        fi
      fi
    fi
    
    # Detect WiFi chipset for potential firmware needs
    WIFI_CHIPSET=""
    WIFI_NEEDS_FIRMWARE=false
    while IFS= read -r line; do
      if echo "$line" | grep -qi "Intel.*Wireless\|Intel.*Wi-Fi\|Intel.*AX"; then
        WIFI_CHIPSET="intel"
      elif echo "$line" | grep -qi "Qualcomm\|Atheros"; then
        WIFI_CHIPSET="qualcomm"
        # Qualcomm WiFi on ARM often needs firmware extraction
        [[ "$IS_ARM" == "true" ]] && WIFI_NEEDS_FIRMWARE=true
      elif echo "$line" | grep -qi "Broadcom"; then
        WIFI_CHIPSET="broadcom"
      elif echo "$line" | grep -qi "Realtek"; then
        WIFI_CHIPSET="realtek"
      elif echo "$line" | grep -qi "MediaTek"; then
        WIFI_CHIPSET="mediatek"
      fi
    done < <(lspci 2>/dev/null | grep -i "network\|wireless\|wifi\|802.11")
    
    # ============================================================
    # Model-specific nixos-hardware detection
    # ============================================================
    
    NIXOS_HARDWARE_MODULE=""
    
    # Use multiple sources to identify the machine
    MACHINE_ID="$SYS_VENDOR $PRODUCT_NAME $PRODUCT_FAMILY $BOARD_NAME $CPU_MODEL"
    
    # ThinkPad detection (comprehensive)
    if echo "$PRODUCT_NAME" | grep -qi "ThinkPad"; then
      MODEL=$(echo "$PRODUCT_NAME" | grep -oP 'ThinkPad\s+\S+' | tr '[:upper:]' '[:lower:]' | tr ' ' '/')
      case "$PRODUCT_NAME" in
        *"T14s Gen 6"*|*"T14s G6"*)
          if [[ "$IS_SNAPDRAGON" == "true" ]]; then
            NIXOS_HARDWARE_MODULE="lenovo-thinkpad-t14s-aarch64"
            log "Detected: ThinkPad T14s Gen 6 (Snapdragon)"
          fi
          ;;
        *"T480"*) NIXOS_HARDWARE_MODULE="lenovo/thinkpad/t480" ;;
        *"T490"*) NIXOS_HARDWARE_MODULE="lenovo/thinkpad/t490" ;;
        *"T14"*) NIXOS_HARDWARE_MODULE="lenovo/thinkpad/t14" ;;
        *"X1 Carbon"*|*"X1C"*)
          if echo "$PRODUCT_NAME" | grep -qi "Gen 9\|9th"; then
            NIXOS_HARDWARE_MODULE="lenovo/thinkpad/x1-carbon/9th-gen"
          elif echo "$PRODUCT_NAME" | grep -qi "Gen 10\|10th"; then
            NIXOS_HARDWARE_MODULE="lenovo/thinkpad/x1-carbon/10th-gen"
          fi
          ;;
        *"X220"*) NIXOS_HARDWARE_MODULE="lenovo/thinkpad/x220" ;;
        *"X230"*) NIXOS_HARDWARE_MODULE="lenovo/thinkpad/x230" ;;
      esac
    
    # Framework detection
    elif echo "$SYS_VENDOR" | grep -qi "Framework"; then
      if echo "$PRODUCT_NAME" | grep -qi "13"; then
        NIXOS_HARDWARE_MODULE="framework/13-inch/common"
      elif echo "$PRODUCT_NAME" | grep -qi "16"; then
        NIXOS_HARDWARE_MODULE="framework/16-inch"
      fi
      log "Detected: Framework laptop"
    
    # Dell XPS detection
    elif echo "$PRODUCT_NAME" | grep -qi "XPS"; then
      case "$PRODUCT_NAME" in
        *"13 9310"*) NIXOS_HARDWARE_MODULE="dell/xps/13-9310" ;;
        *"13 9380"*) NIXOS_HARDWARE_MODULE="dell/xps/13-9380" ;;
        *"15 9500"*) NIXOS_HARDWARE_MODULE="dell/xps/15-9500" ;;
        *"15 9510"*) NIXOS_HARDWARE_MODULE="dell/xps/15-9510" ;;
      esac
      log "Detected: Dell XPS"
    
    # Yoga Slim 7x (Snapdragon)
    elif echo "$PRODUCT_NAME" | grep -qi "Yoga Slim 7x\|83ED"; then
      if [[ "$IS_SNAPDRAGON" == "true" ]]; then
        NIXOS_HARDWARE_MODULE="lenovo-yoga-slim7x-snapdragon"
        log "Detected: Lenovo Yoga Slim 7x (Snapdragon)"
      fi
    
    # Surface devices
    elif echo "$SYS_VENDOR" | grep -qi "Microsoft"; then
      if echo "$PRODUCT_NAME" | grep -qi "Surface"; then
        log "Detected: Microsoft Surface - may need linux-surface kernel"
        # TODO: Could add linux-surface flake input here
      fi
    
    # ASUS detection
    elif echo "$SYS_VENDOR" | grep -qi "ASUS\|ASUSTeK"; then
      if echo "$PRODUCT_NAME" | grep -qi "ROG"; then
        NIXOS_HARDWARE_MODULE="asus/rog-strix"
        log "Detected: ASUS ROG"
      elif echo "$PRODUCT_NAME" | grep -qi "Zephyrus"; then
        NIXOS_HARDWARE_MODULE="asus/zephyrus"
        log "Detected: ASUS Zephyrus"
      fi
    
    # HP detection  
    elif echo "$SYS_VENDOR" | grep -qi "HP\|Hewlett"; then
      if echo "$PRODUCT_NAME" | grep -qi "EliteBook"; then
        log "Detected: HP EliteBook"
        # HP EliteBooks generally work well, no special module needed
      elif echo "$PRODUCT_NAME" | grep -qi "Spectre"; then
        log "Detected: HP Spectre"
      fi
    
    # Apple Silicon (if someone manages to run NixOS on it)
    elif echo "$SYS_VENDOR" | grep -qi "Apple" || [[ "$CPU_VENDOR" == "apple" ]]; then
      log "Detected: Apple hardware - may need Asahi Linux kernel"
      # TODO: Could add asahi flake input
    
    # System76 - excellent Linux support
    elif echo "$SYS_VENDOR" | grep -qi "System76"; then
      NIXOS_HARDWARE_MODULE="system76"
      log "Detected: System76 - excellent NixOS support"
    
    # Purism Librem
    elif echo "$SYS_VENDOR" | grep -qi "Purism"; then
      log "Detected: Purism Librem"
    
    # Tuxedo
    elif echo "$SYS_VENDOR" | grep -qi "TUXEDO"; then
      log "Detected: TUXEDO - may benefit from tuxedo-control-center"
    fi
    
    # Additional detection via CPU for machines where DMI isn't helpful
    if [[ -z "$NIXOS_HARDWARE_MODULE" ]]; then
      # Intel CPU generation detection for generic optimizations
      if [[ "$CPU_VENDOR" == "intel" ]]; then
        if echo "$CPU_MODEL" | grep -qiE "11th Gen|1[12][0-9]{2}"; then
          log "Detected: Intel 11th Gen (Tiger Lake)"
        elif echo "$CPU_MODEL" | grep -qiE "12th Gen|12[0-9]{2}"; then
          log "Detected: Intel 12th Gen (Alder Lake)"
        elif echo "$CPU_MODEL" | grep -qiE "13th Gen|13[0-9]{2}"; then
          log "Detected: Intel 13th Gen (Raptor Lake)"  
        elif echo "$CPU_MODEL" | grep -qiE "14th Gen|14[0-9]{2}|Core Ultra"; then
          log "Detected: Intel 14th Gen / Core Ultra (Meteor Lake)"
        fi
      elif [[ "$CPU_VENDOR" == "amd" ]]; then
        if echo "$CPU_MODEL" | grep -qiE "Ryzen.*5[0-9]{3}"; then
          log "Detected: AMD Ryzen 5000 series (Zen 3)"
        elif echo "$CPU_MODEL" | grep -qiE "Ryzen.*6[0-9]{3}"; then
          log "Detected: AMD Ryzen 6000 series (Zen 3+)"
        elif echo "$CPU_MODEL" | grep -qiE "Ryzen.*7[0-9]{3}"; then
          log "Detected: AMD Ryzen 7000 series (Zen 4)"
        elif echo "$CPU_MODEL" | grep -qiE "Ryzen.*8[0-9]{3}|Ryzen AI"; then
          log "Detected: AMD Ryzen 8000/AI series (Zen 4/5)"
        fi
      fi
    fi
    
    # Log detection results
    log "Hardware Detection Results:"
    log "  Vendor: $SYS_VENDOR"
    log "  Product: $PRODUCT_NAME"
    log "  CPU: $CPU_MODEL"
    log "  RAM: ''${RAM_GB}GB (swap recommendation: ''${SWAP_SIZE_MB}MB)"
    log "  Architecture: $ARCH"
    log "  Is Laptop: $IS_LAPTOP"
    log "  Is ARM/Snapdragon: $IS_ARM / $IS_SNAPDRAGON"
    log "  NVIDIA GPU: $HAS_NVIDIA''${NVIDIA_BUS_ID:+ (bus $NVIDIA_BUS_ID)}''${NVIDIA_MODEL:+ - $NVIDIA_MODEL}"
    log "  AMD GPU: $HAS_AMD_GPU''${AMD_BUS_ID:+ (bus $AMD_BUS_ID)}"
    log "  Intel GPU: $HAS_INTEL_GPU''${INTEL_BUS_ID:+ (bus $INTEL_BUS_ID)}"
    log "  WiFi: ''${WIFI_CHIPSET:-unknown}''${WIFI_NEEDS_FIRMWARE:+ (may need firmware)}"
    log "  nixos-hardware module: ''${NIXOS_HARDWARE_MODULE:-none detected}"
    
    # Generate hardware detection report for config generation
    cat > "$HARDWARE_REPORT" << EOF
{
  "vendor": "$SYS_VENDOR",
  "product": "$PRODUCT_NAME",
  "product_family": "$PRODUCT_FAMILY",
  "board_name": "$BOARD_NAME",
  "cpu_model": "$CPU_MODEL",
  "cpu_vendor": "$CPU_VENDOR",
  "ram_mb": $RAM_MB,
  "ram_gb": $RAM_GB,
  "swap_recommended_mb": $SWAP_SIZE_MB,
  "arch": "$ARCH",
  "is_laptop": $IS_LAPTOP,
  "is_arm": $IS_ARM,
  "is_snapdragon": $IS_SNAPDRAGON,
  "snapdragon_model": "$SNAPDRAGON_MODEL",
  "has_nvidia": $HAS_NVIDIA,
  "nvidia_bus_id": "$NVIDIA_BUS_ID",
  "nvidia_model": "$NVIDIA_MODEL",
  "has_amd_gpu": $HAS_AMD_GPU,
  "amd_bus_id": "$AMD_BUS_ID",
  "has_intel_gpu": $HAS_INTEL_GPU,
  "intel_bus_id": "$INTEL_BUS_ID",
  "wifi_chipset": "$WIFI_CHIPSET",
  "wifi_needs_firmware": $WIFI_NEEDS_FIRMWARE,
  "nixos_hardware_module": "$NIXOS_HARDWARE_MODULE"
}
EOF
    
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
        
        # Build flake inputs based on detected hardware
        NIXOS_HARDWARE_INPUT=""
        NIXOS_HARDWARE_MODULE_IMPORT=""
        SNAPDRAGON_INPUT=""
        SNAPDRAGON_MODULE_IMPORT=""
        
        if [[ -n "$NIXOS_HARDWARE_MODULE" ]]; then
          # Special handling for Snapdragon laptops
          if [[ "$NIXOS_HARDWARE_MODULE" == "lenovo-yoga-slim7x-snapdragon" ]]; then
            SNAPDRAGON_INPUT='
    # Snapdragon X Elite support
    x1e-nixos = {
      url = "github:kuruczgy/x1e-nixos-config";
      inputs.nixpkgs.follows = "nixpkgs";
    };'
            SNAPDRAGON_MODULE_IMPORT='x1e-nixos.nixosModules.yoga-slim7x'
            log "Adding Snapdragon X Elite (Yoga Slim 7x) support"
          elif [[ "$NIXOS_HARDWARE_MODULE" == "lenovo-thinkpad-t14s-aarch64" ]]; then
            SNAPDRAGON_INPUT='
    # Snapdragon X Elite support
    x1e-nixos = {
      url = "github:kuruczgy/x1e-nixos-config";
      inputs.nixpkgs.follows = "nixpkgs";
    };'
            SNAPDRAGON_MODULE_IMPORT='x1e-nixos.nixosModules.thinkpad-t14s'
            log "Adding Snapdragon X Elite (ThinkPad T14s) support"
          else
            # Standard nixos-hardware module
            NIXOS_HARDWARE_INPUT='
    nixos-hardware.url = "github:NixOS/nixos-hardware";'
            NIXOS_HARDWARE_MODULE_IMPORT="nixos-hardware.nixosModules.$NIXOS_HARDWARE_MODULE"
            log "Adding nixos-hardware module: $NIXOS_HARDWARE_MODULE"
          fi
        fi
        
        # Add laptop module config if detected as laptop
        LAPTOP_CONFIG=""
        if [[ "$IS_LAPTOP" == "true" ]]; then
          LAPTOP_CONFIG='
          # Laptop optimizations
          jch.laptop.enable = true;'
          
          # Add NVIDIA config if hybrid graphics detected
          if [[ "$HAS_NVIDIA" == "true" && ( "$HAS_INTEL_GPU" == "true" || "$HAS_AMD_GPU" == "true" ) ]]; then
            IGPU_BUS="$INTEL_BUS_ID"
            [[ -z "$IGPU_BUS" ]] && IGPU_BUS="$AMD_BUS_ID"
            LAPTOP_CONFIG="$LAPTOP_CONFIG"'
          jch.laptop.nvidia = true;
          hardware.nvidia.prime.intelBusId = "PCI:'"$IGPU_BUS"'";
          hardware.nvidia.prime.nvidiaBusId = "PCI:'"$NVIDIA_BUS_ID"'";'
            log "Configured NVIDIA Optimus: Intel/AMD=$IGPU_BUS, NVIDIA=$NVIDIA_BUS_ID"
          fi
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
    };$NIXOS_HARDWARE_INPUT$SNAPDRAGON_INPUT
  };

  outputs = { self, nixpkgs, home-manager, ... }@inputs: 
  let
    mkHost = { 
      system, 
      hostname, 
      username ? "$USERNAME",
      extraModules ? [],
    }: nixpkgs.lib.nixosSystem {
      inherit system;
      specialArgs = { inherit self username inputs; };
      modules = [
        home-manager.nixosModules.home-manager
        ./modules/common.nix
        {
          networking.hostName = hostname;
          home-manager.users.\''${username} = import ./home;
          home-manager.extraSpecialArgs = { inherit username; };$LAPTOP_CONFIG
        }
      ] ++ extraModules;
    };
  in
  {
    nixosConfigurations = {
      # $HOSTNAME - installed $(date +%Y-%m-%d)
      # Hardware: $SYS_VENDOR $PRODUCT_NAME
      "$HOSTNAME" = mkHost {
        system = "$SYSTEM";
        hostname = "$HOSTNAME";
        username = "$USERNAME";
        extraModules = [
          ./hosts/$HOSTNAME''${NIXOS_HARDWARE_MODULE_IMPORT:+
          inputs.$NIXOS_HARDWARE_MODULE_IMPORT}''${SNAPDRAGON_MODULE_IMPORT:+
          inputs.$SNAPDRAGON_MODULE_IMPORT}
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
        
        # Add auto-detected swap configuration for hibernate support
        if [[ "$IS_LAPTOP" == "true" && -f "$HWCONF_DEST" ]]; then
          log "Adding swap configuration for hibernate (''${SWAP_SIZE_MB}MB based on ''${RAM_GB}GB RAM)..."
          
          # Check if swapDevices is already configured
          if ! grep -q "swapDevices" "$HWCONF_DEST"; then
            # Add swap configuration before the closing brace
            # Use a swapfile for simplicity (works with loopback and partition installs)
            cat >> "$HWCONF_DEST" << SWAPEOF

  # Auto-configured swap for hibernate support
  # Size: ''${SWAP_SIZE_MB}MB (RAM + 2GB buffer for hibernate)
  swapDevices = [{
    device = "/swapfile";
    size = $SWAP_SIZE_MB;
  }];
SWAPEOF
            log "Swap configuration added to hardware-configuration.nix"
          else
            log "swapDevices already configured, skipping auto-swap"
          fi
        fi
        
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
      # Starter config uses the actual hostname as the configuration name
      INSTALL_HOSTNAME="$HOSTNAME"
    elif [[ "$FLAKE_TYPE" == "minimal" ]]; then
      # Minimal config uses 'nixos' as the configuration name
      INSTALL_HOSTNAME="nixos"
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
    
    # Create first-boot hibernate configuration script for laptops
    if [[ "$IS_LAPTOP" == "true" && -f /mnt/swapfile ]] || [[ "$IS_LAPTOP" == "true" ]]; then
      cat > /mnt/usr/local/bin/setup-hibernate << 'HIBERNATE_SCRIPT'
#!/usr/bin/env bash
# Auto-configure hibernate resume parameters
# Run this after first boot to enable hibernate with suspend-then-hibernate

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "This script requires root privileges."
  exec sudo "$0" "$@"
fi

SWAPFILE="/swapfile"
CONFIG_DIR="/etc/nixos"

if [[ ! -f "$SWAPFILE" ]]; then
  echo "No swapfile found at $SWAPFILE"
  echo "Creating swapfile first..."
  # Swapfile should have been created by installer
  exit 1
fi

echo "Configuring hibernate resume parameters..."

# Get the device containing the swapfile
RESUME_DEVICE=$(df "$SWAPFILE" | tail -1 | awk '{print $1}')
RESUME_UUID=$(blkid -s UUID -o value "$RESUME_DEVICE")

# Get the physical offset of the swapfile
RESUME_OFFSET=$(filefrag -v "$SWAPFILE" | awk 'NR==4 {print $4}' | sed 's/\.\.//')

if [[ -z "$RESUME_OFFSET" ]]; then
  echo "Could not determine swapfile offset. Is filefrag available?"
  exit 1
fi

echo "Resume device: /dev/disk/by-uuid/$RESUME_UUID"
echo "Resume offset: $RESUME_OFFSET"

# Find the hardware-configuration.nix file
HWCONF=""
for f in "$CONFIG_DIR/hardware-configuration.nix" \
         "$CONFIG_DIR"/hosts/*/hardware-configuration.nix; do
  if [[ -f "$f" ]]; then
    HWCONF="$f"
    break
  fi
done

if [[ -z "$HWCONF" ]]; then
  echo "Could not find hardware-configuration.nix"
  echo ""
  echo "Add these manually to your configuration:"
  echo "  boot.resumeDevice = \"/dev/disk/by-uuid/$RESUME_UUID\";"
  echo "  boot.kernelParams = [ \"resume_offset=$RESUME_OFFSET\" ];"
  exit 1
fi

echo "Updating $HWCONF..."

# Check if already configured
if grep -q "resumeDevice" "$HWCONF" && grep -q "resume_offset" "$HWCONF"; then
  echo "Hibernate already configured!"
  exit 0
fi

# Add resume configuration
if ! grep -q "resumeDevice" "$HWCONF"; then
  # Add before the closing brace
  sed -i '/^}$/i\
  # Hibernate/resume configuration (auto-generated)\
  boot.resumeDevice = "/dev/disk/by-uuid/'"$RESUME_UUID"'";\
  boot.kernelParams = [ "resume_offset='"$RESUME_OFFSET"'" ];' "$HWCONF"
fi

echo ""
echo "✓ Hibernate configured!"
echo ""
echo "Rebuilding NixOS configuration..."
nixos-rebuild switch

echo ""
echo "Hibernate is now ready. Test with: systemctl hibernate"
echo "Suspend-then-hibernate will work automatically with jch.laptop.enable"
HIBERNATE_SCRIPT
      chmod +x /mnt/usr/local/bin/setup-hibernate
      log "Created /usr/local/bin/setup-hibernate helper script"
      
      # Also create a systemd service to run this on first boot
      mkdir -p /mnt/etc/systemd/system
      cat > /mnt/etc/systemd/system/setup-hibernate.service << 'SYSTEMD_UNIT'
[Unit]
Description=Configure hibernate resume parameters
After=local-fs.target
ConditionPathExists=/swapfile
ConditionPathExists=!/var/lib/hibernate-configured

[Service]
Type=oneshot
ExecStart=/usr/local/bin/setup-hibernate
ExecStartPost=/usr/bin/touch /var/lib/hibernate-configured
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
SYSTEMD_UNIT
      # Enable for first boot
      mkdir -p /mnt/etc/systemd/system/multi-user.target.wants
      ln -sf ../setup-hibernate.service /mnt/etc/systemd/system/multi-user.target.wants/
      log "Hibernate will be auto-configured on first boot"
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
let
  nixosSystem = pkgs.nixos {
    imports = [
      "${pkgs.path}/nixos/modules/profiles/minimal.nix"
      "${pkgs.path}/nixos/modules/profiles/all-hardware.nix"
    ];
    
    config = {
      # Basic system config
      system.stateVersion = "24.11";
      
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
  };
in {
  # The toplevel (for compatibility)
  toplevel = nixosSystem.config.system.build.toplevel;
  
  # Individual components
  kernel = nixosSystem.config.system.build.kernel;
  initrd = nixosSystem.config.system.build.initialRamdisk;
  
  # Combined boot assets
  bootAssets = pkgs.runCommand "installer-boot-assets" {
    nativeBuildInputs = [ pkgs.coreutils ];
  } ''
    mkdir -p $out
    cp ${nixosSystem.config.system.build.kernel}/*Image $out/bzImage 2>/dev/null || \
      cp ${nixosSystem.config.system.build.kernel}/bzImage $out/bzImage
    cp ${nixosSystem.config.system.build.initialRamdisk}/initrd $out/initrd
    
    # Export the init path - required for booting NixOS
    # This is the path to stage-2 init in the toplevel closure
    echo "${nixosSystem.config.system.build.toplevel}/init" > $out/init-path
    
    cd $out
    sha256sum bzImage initrd init-path > SHA256SUMS
  '';
  
  # Default is the toplevel for backwards compatibility
  default = nixosSystem.config.system.build.toplevel;
}
