{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  name = "webgpu-browser-env";

  buildInputs = with pkgs; [
    # Browsers with WebGPU support
    chromium          # Standard Chromium (faster to build than ungoogled)
    firefox           # Firefox with WebGPU flags

    # Vulkan support (required for WebGPU on Linux)
    vulkan-tools      # vulkaninfo, vkcube
    vulkan-loader     # Vulkan ICD loader
    vulkan-headers    # Vulkan headers
    vulkan-validation-layers  # Debugging layers

    # Mesa drivers (GPU acceleration)
    mesa              # OpenGL/Vulkan drivers
    mesa.drivers      # DRI drivers

    # Additional tools
    glxinfo           # Check OpenGL info
    pciutils          # lspci for GPU detection
  ];

  shellHook = ''
    echo "═══════════════════════════════════════════════════════════"
    echo "  WebGPU Browser Environment"
    echo "═══════════════════════════════════════════════════════════"
    echo ""
    echo "GPU Info:"
    lspci | grep -i vga || echo "  (no GPU info available)"
    echo ""
    echo "Vulkan Status:"
    if vulkaninfo --summary 2>/dev/null | head -5; then
      echo "  ✓ Vulkan is available"
    else
      echo "  ✗ Vulkan not detected"
    fi
    echo ""
    echo "Launch commands:"
    echo "  chromium-webgpu   - Chromium with WebGPU enabled"
    echo "  firefox-webgpu    - Firefox with WebGPU enabled"
    echo ""
    echo "Test WebGPU at: http://localhost:8080/benchmark-real.html"
    echo "═══════════════════════════════════════════════════════════"

    # Create wrapper scripts
    alias chromium-webgpu='chromium \
      --enable-unsafe-webgpu \
      --enable-features=Vulkan,UseSkiaRenderer \
      --use-vulkan=native \
      --ignore-gpu-blocklist \
      --enable-gpu-rasterization \
      --enable-zero-copy'

    alias firefox-webgpu='firefox \
      -purgecaches \
      about:config'

    export LIBGL_ALWAYS_SOFTWARE=0
    export MESA_LOADER_DRIVER_OVERRIDE=''${MESA_LOADER_DRIVER_OVERRIDE:-}
  '';

  # Set Vulkan ICD paths
  VK_ICD_FILENAMES = "${pkgs.mesa.drivers}/share/vulkan/icd.d/radeon_icd.x86_64.json:${pkgs.mesa.drivers}/share/vulkan/icd.d/intel_icd.x86_64.json:${pkgs.mesa.drivers}/share/vulkan/icd.d/nvidia_icd.json";
}
