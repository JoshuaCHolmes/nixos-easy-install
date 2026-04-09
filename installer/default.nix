# Nix derivation for building the Windows installer executable
# 
# This cross-compiles the Rust GUI installer to x86_64-pc-windows-gnu target.

{ lib
, rustToolchain
, windowsCross
, stdenv
, pkg-config
, openssl
}:

stdenv.mkDerivation rec {
  pname = "nixos-easy-install";
  version = "0.1.0";
  
  src = ./.;
  
  nativeBuildInputs = [
    rustToolchain
    pkg-config
  ];
  
  buildInputs = [
    openssl
  ];
  
  # Cross-compilation for Windows
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${windowsCross.stdenv.cc}/bin/x86_64-w64-mingw32-gcc";
  
  buildPhase = ''
    export HOME=$TMPDIR
    cargo build --release --target x86_64-pc-windows-gnu
  '';
  
  installPhase = ''
    mkdir -p $out/bin
    cp target/x86_64-pc-windows-gnu/release/nixos-install.exe $out/bin/
  '';
  
  meta = with lib; {
    description = "Windows GUI installer for NixOS dual-boot or standalone";
    homepage = "https://github.com/JoshuaCHolmes/nixos-easy-install";
    license = licenses.mit;
    platforms = [ "x86_64-linux" ];
  };
}
