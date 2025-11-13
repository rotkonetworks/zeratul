#!/bin/bash
# Deployment script for Ligerito WASM
set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Ligerito WASM Deployment Script"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo

# Step 1: Build WASM
echo "Step 1: Building multi-threaded WASM..."
./build-wasm-parallel.sh

echo
echo "Step 2: Copying helper files..."
mkdir -p ../examples/www/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src
cp ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasm-bindgen-rayon-1.3.0/src/workerHelpers.* \
   ../examples/www/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/

echo
echo "Step 3: Creating deployment package..."
mkdir -p deploy
cp -r ../examples/www/* deploy/

# Create _headers file for proper CORS (for Netlify/Cloudflare Pages)
cat > deploy/_headers <<EOF
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Cross-Origin-Resource-Policy: cross-origin
  Access-Control-Allow-Origin: *
EOF

# Create vercel.json for Vercel deployment
cat > deploy/vercel.json <<EOF
{
  "headers": [
    {
      "source": "/(.*)",
      "headers": [
        {
          "key": "Cross-Origin-Opener-Policy",
          "value": "same-origin"
        },
        {
          "key": "Cross-Origin-Embedder-Policy",
          "value": "require-corp"
        },
        {
          "key": "Cross-Origin-Resource-Policy",
          "value": "cross-origin"
        }
      ]
    }
  ]
}
EOF

# Create netlify.toml for Netlify deployment
cat > deploy/netlify.toml <<EOF
[[headers]]
  for = "/*"
  [headers.values]
    Cross-Origin-Opener-Policy = "same-origin"
    Cross-Origin-Embedder-Policy = "require-corp"
    Cross-Origin-Resource-Policy = "cross-origin"
EOF

echo
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Deployment package ready in ./deploy/"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo
echo "Deployment options:"
echo
echo "1. GitHub Pages:"
echo "   - Create gh-pages branch"
echo "   - Copy deploy/* to gh-pages branch"
echo "   - Push to GitHub"
echo "   - Enable GitHub Pages in repo settings"
echo
echo "2. Netlify:"
echo "   - Run: cd deploy && netlify deploy --prod"
echo "   - Or drag ./deploy folder to netlify.com/drop"
echo
echo "3. Vercel:"
echo "   - Run: cd deploy && vercel --prod"
echo
echo "4. Cloudflare Pages:"
echo "   - Upload ./deploy folder via dashboard"
echo
echo "Note: All platforms need SharedArrayBuffer support enabled."
echo "The _headers, vercel.json, and netlify.toml files are included."
echo
