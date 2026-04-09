{ pkgs ? import <nixpkgs> {} }:

let
  # Windows cross-compilation toolchain
  mingw = pkgs.pkgsCross.mingwW64;
  pthreadsLib = "${mingw.windows.pthreads}/lib";
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    rustup
    mingw.buildPackages.gcc
  ];

  # Tell cargo where to find the linker for Windows target
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${mingw.buildPackages.gcc}/bin/x86_64-w64-mingw32-gcc";
  
  # For some crates with build scripts
  CC = "${mingw.buildPackages.gcc}/bin/x86_64-w64-mingw32-gcc";
  
  # pthreads library path (needed for final link)
  PTHREADS_LIB = pthreadsLib;

  shellHook = ''
    echo "NixOS Easy Install - Development Shell"
    echo ""
    echo "Build commands:"
    echo "  cargo build                                    # Native build (for testing)"
    echo "  cargo build --release --target x86_64-pc-windows-gnu  # Windows exe"
    echo ""
    echo "Note: Windows build requires .cargo/config.toml with pthreads path:"
    echo "  PTHREADS_LIB=${pthreadsLib}"
    echo ""
    
    # Ensure rustup has the Windows target
    rustup target add x86_64-pc-windows-gnu 2>/dev/null || true
  '';
}
