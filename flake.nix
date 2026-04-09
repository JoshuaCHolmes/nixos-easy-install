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
              echo "  nix build .#initrd                          # Build installer initrd"
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

      # The Windows installer executable (x86_64-linux only for actual cross-compilation)
      packages.x86_64-linux = 
        let
          pkgs = mkPkgs "x86_64-linux";
          rustToolchain = mkRustToolchain pkgs;
          windowsCross = pkgs.pkgsCross.mingwW64;
        in {
          installer = pkgs.callPackage ./installer { inherit rustToolchain windowsCross; };
          initrd = pkgs.callPackage ./initrd { };
          
          default = self.packages.x86_64-linux.installer;
        };

      # Installer initrd as a NixOS module for testing
      nixosModules.installer = import ./initrd/module.nix;
    };
}
