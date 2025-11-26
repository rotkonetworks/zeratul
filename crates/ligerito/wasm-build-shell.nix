{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  name = "ligerito-wasm-build";

  buildInputs = with pkgs; [
    # Rust with wasm32 target via rustup
    rustup

    # wasm-bindgen CLI for generating JS bindings
    wasm-bindgen-cli

    # wasm-pack for building (alternative to raw wasm-bindgen)
    wasm-pack

    # binaryen for wasm-opt
    binaryen

    # Node.js for testing
    nodejs
  ];

  shellHook = ''
    echo "═══════════════════════════════════════════════════════════"
    echo "  Ligerito WASM Build Environment"
    echo "═══════════════════════════════════════════════════════════"
    echo ""

    # Set up rustup in a local directory to not conflict with system rust
    export RUSTUP_HOME="$PWD/.rustup"
    export CARGO_HOME="$PWD/.cargo"
    export PATH="$CARGO_HOME/bin:$PATH"

    # Install stable toolchain and wasm target if not present
    if [ ! -f "$CARGO_HOME/bin/cargo" ]; then
      echo "Installing Rust toolchain..."
      rustup-init -y --no-modify-path --default-toolchain stable
      rustup target add wasm32-unknown-unknown
    fi

    echo "Rust version: $(cargo --version)"
    echo "wasm-bindgen: $(wasm-bindgen --version)"
    echo ""
    echo "Build commands:"
    echo "  wasm-pack build --target web --features wasm    # Full build"
    echo "  wasm-pack build --target web --features wasm-lite --no-default-features  # Lite"
    echo ""
    echo "═══════════════════════════════════════════════════════════"
  '';
}
