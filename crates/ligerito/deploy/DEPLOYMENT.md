# Ligerito WASM Deployment Guide

This directory contains a ready-to-deploy Ligerito WASM prover/verifier application.

## Requirements

**IMPORTANT**: This application uses `SharedArrayBuffer` for multi-threading, which requires specific HTTP headers:
- `Cross-Origin-Opener-Policy: same-origin`
- `Cross-Origin-Embedder-Policy: require-corp`

All deployment configurations in this directory include these headers automatically.

## Deployment Options

### 1. Netlify (Recommended)

**Option A: Drag & Drop**
1. Go to https://app.netlify.com/drop
2. Drag the entire `deploy/` folder
3. Done! Your site is live

**Option B: CLI**
```bash
npm install -g netlify-cli
cd deploy
netlify deploy --prod
```

The `netlify.toml` file is already configured with the required headers.

### 2. Vercel

```bash
npm install -g vercel
cd deploy
vercel --prod
```

The `vercel.json` file is already configured with the required headers.

### 3. Cloudflare Pages

1. Go to https://dash.cloudflare.com/
2. Pages → Create a project → Upload assets
3. Upload the `deploy/` folder

The `_headers` file is already configured with the required headers.

### 4. GitHub Pages

```bash
# Create gh-pages branch
git checkout --orphan gh-pages
git rm -rf .
cp -r path/to/deploy/* .
git add .
git commit -m "Deploy Ligerito WASM"
git push origin gh-pages
```

Then enable GitHub Pages in repository settings (Settings → Pages → Source: gh-pages branch).

**Note**: GitHub Pages may require additional configuration for CORS headers. Consider using Netlify or Vercel instead.

### 5. Custom Server

If deploying to a custom server, ensure your web server sends the required headers:

**Nginx:**
```nginx
add_header Cross-Origin-Opener-Policy same-origin;
add_header Cross-Origin-Embedder-Policy require-corp;
add_header Cross-Origin-Resource-Policy cross-origin;
```

**Apache (.htaccess):**
```apache
Header set Cross-Origin-Opener-Policy "same-origin"
Header set Cross-Origin-Embedder-Policy "require-corp"
Header set Cross-Origin-Resource-Policy "cross-origin"
```

## File Structure

```
deploy/
├── index.html              # Main application
├── ligerito.js             # WASM bindings
├── ligerito_bg.wasm        # WASM module (330 KB)
├── worker.js               # Web Worker for proving
├── style.css               # Styles
├── snippets/               # wasm-bindgen-rayon helpers
├── _headers                # Cloudflare Pages headers
├── netlify.toml            # Netlify configuration
└── vercel.json             # Vercel configuration
```

## Features

- **Multi-threaded WASM**: Uses Rayon for parallel computation via Web Workers
- **SIMD Optimized**: Compiled with SIMD128 support for 2-4x speedup
- **Parallel Sumcheck**: Uses chunked parallelism for optimal performance
- **Three config sizes**: 2^12 (4 KB), 2^20 (4 MB), 2^24 (64 MB)

## Performance

Approximate proving times on modern hardware (2^20 polynomial):
- **WASM (browser)**: 30-40 seconds
- **Native (Rust)**: 8-10 seconds

The 3-5x slowdown compared to native is expected for WASM.

## Testing Locally

```bash
cd deploy
python3 serve-multithreaded.py 8080
# Open http://localhost:8080
```

## Troubleshooting

**SharedArrayBuffer not available:**
- Check browser console for CORS errors
- Verify your hosting platform supports custom headers
- Test with: `python3 serve-multithreaded.py 8080` locally

**Slow performance:**
- Ensure browser supports SharedArrayBuffer (Chrome 68+, Firefox 79+)
- Check browser console for thread pool initialization messages
- Try different config sizes (2^12 is fastest for testing)

**Module loading errors:**
- Clear browser cache
- Check that all files (.wasm, .js, snippets/) are uploaded
- Verify MIME types are set correctly (see serve-multithreaded.py)

## Browser Compatibility

✅ Chrome 68+
✅ Firefox 79+
✅ Edge 79+
✅ Safari 15.2+

**Note**: Safari requires macOS 11.3+ or iOS 14.5+ for SharedArrayBuffer support.

## Security Note

This application runs cryptographic proofs entirely in your browser. No data is sent to any server. The WASM module is deterministic and reproducible from source.
