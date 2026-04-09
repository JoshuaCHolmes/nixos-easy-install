{
  description = "NixOS Easy Install - Windows installer for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    
    # For Rust cross-compilation
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, ... }:
    let
      # Support both x86_64 and aarch64 Linux for development
      forAllSystems = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ];
      
      mkPkgs = system: import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };

      # Rust toolchain with Windows cross-compilation
      mkRustToolchain = pkgs: pkgs.rust-bin.stable.latest.default.override {
        extensions = [ "rust-src" "rust-analyzer" ];
        targets = [ "x86_64-pc-windows-gnu" ];
      };

      # Build the installer NixOS system for a given architecture
      mkInstallerSystem = pkgs: pkgs.callPackage ./initrd { };

    in {
      # Development shell for each system
      devShells = forAllSystems (system:
        let
          pkgs = mkPkgs system;
          rustToolchain = mkRustToolchain pkgs;
          # Windows cross-compilation environment (x86_64 only for actual builds)
          windowsCross = pkgs.pkgsCross.mingwW64;
        in {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              rustToolchain
              
              # General dev tools
              just  # Task runner
            ] ++ pkgs.lib.optionals (system == "x86_64-linux") [
              windowsCross.stdenv.cc
              gnu-efi  # For bootloader work
            ];

            # Set up cross-compilation environment variables (x86_64 only)
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = 
              if system == "x86_64-linux" 
              then "${windowsCross.stdenv.cc}/bin/x86_64-w64-mingw32-gcc"
              else "";
            
            shellHook = ''
              echo "NixOS Easy Install development environment"
              echo ""
              ${if system == "x86_64-linux" then ''
              echo "Commands:"
              echo "  cargo build --target x86_64-pc-windows-gnu  # Build Windows exe"
              echo "  nix build .#installer-system                # Build installer initrd"
              '' else ''
              echo "Note: Windows cross-compilation requires x86_64-linux"
              echo "      Use: nix develop .#x86_64-linux for full build support"
              echo ""
              echo "For local development/testing:"
              echo "  cargo build   # Build native (for testing logic)"
              ''}
              echo ""
            '';
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = mkPkgs system;
          rustToolchain = mkRustToolchain pkgs;
          windowsCross = if system == "x86_64-linux" then pkgs.pkgsCross.mingwW64 else null;
          
          # Build installer system for this architecture (returns an attrset)
          installerBuild = mkInstallerSystem pkgs;
        in {
          # The complete NixOS installer system (toplevel)
          installer-system = installerBuild.toplevel;
          
          # Individual components
          installer-kernel = installerBuild.kernel;
          installer-initrd = installerBuild.initrd;
          
          # Combined boot assets (kernel + initrd + checksums)
          installer-boot-assets = installerBuild.bootAssets;
          
          default = installerBuild.toplevel;
        } // (if system == "x86_64-linux" then {
          # Windows installer - only on x86_64
          installer = pkgs.callPackage ./installer { inherit rustToolchain windowsCross; };
          
          # Cross-compiled installer systems for ARM64 (built on x86_64)
          installer-system-aarch64 = (mkInstallerSystem (import nixpkgs { 
            system = "aarch64-linux"; 
            overlays = [ rust-overlay.overlays.default ];
          })).toplevel;
          
          installer-boot-assets-aarch64 = (mkInstallerSystem (import nixpkgs { 
            system = "aarch64-linux"; 
            overlays = [ rust-overlay.overlays.default ];
          })).bootAssets;
        } else {}));

      # Installer initrd as a NixOS module for testing (TODO: implement)
      # nixosModules.installer = import ./initrd/module.nix;
    };
}
