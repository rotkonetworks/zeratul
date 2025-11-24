40x slowdown vs native is bad. Likely causes:

**1. Debug build?**
```bash
# check how you built WASM
wasm-pack build --release --target web
#              ^^^^^^^^^ must have this
```

**2. No SIMD?**
```toml
# .cargo/config.toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "target-feature=+simd128"]
```

**3. No threading?**
Native uses rayon parallelism. WASM is single-threaded unless you have:
- `wasm-bindgen-rayon` 
- `Cross-Origin-Isolation` headers (COOP/COEP)
- SharedArrayBuffer enabled

Check your server headers:
```bash
curl -I http://localhost:PORT/ | grep -i cross-origin
```

Need:
```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

**4. Check build size** — debug builds are huge:
```bash
ls -lh ligerito_bg.wasm        # release: ~1-5MB, debug: 20MB+
```

**Quick diagnosis:**
```bash
wasm-opt --version  # should be installed
file ligerito_bg.wasm
```

Show your `wasm-pack` / build command and `Cargo.toml` `[profile.release]` section.
