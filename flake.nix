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
      # We build on Linux, target Windows
      buildSystem = "x86_64-linux";
      
      pkgs = import nixpkgs {
        system = buildSystem;
        overlays = [ rust-overlay.overlays.default ];
      };

      # Rust toolchain with Windows cross-compilation
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        extensions = [ "rust-src" "rust-analyzer" ];
        targets = [ "x86_64-pc-windows-gnu" ];
      };

      # Windows cross-compilation environment
      windowsCross = pkgs.pkgsCross.mingwW64;

    in {
      # Development shell
      devShells.${buildSystem}.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustToolchain
          windowsCross.stdenv.cc
          
          # For bootloader work
          gnu-efi
          
          # General dev tools
          just  # Task runner
        ];

        # Set up cross-compilation environment variables
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${windowsCross.stdenv.cc}/bin/x86_64-w64-mingw32-gcc";
        
        shellHook = ''
          echo "NixOS Easy Install development environment"
          echo ""
          echo "Commands:"
          echo "  cargo build --target x86_64-pc-windows-gnu  # Build Windows exe"
          echo "  nix build .#initrd                          # Build installer initrd"
          echo ""
        '';
      };

      # The Windows installer executable
      packages.${buildSystem} = {
        installer = pkgs.callPackage ./installer { inherit rustToolchain windowsCross; };
        initrd = pkgs.callPackage ./initrd { };
        
        default = self.packages.${buildSystem}.installer;
      };

      # Installer initrd as a NixOS module for testing
      nixosModules.installer = import ./initrd/module.nix;
    };
}
