#!/usr/bin/env python3
"""
HTTP server with CORS headers required for SharedArrayBuffer and multi-threaded WASM.

This server sets:
- Cross-Origin-Opener-Policy: same-origin
- Cross-Origin-Embedder-Policy: require-corp

These headers are required for:
- SharedArrayBuffer
- Atomics
- Multi-threaded WASM with wasm-bindgen-rayon

Usage:
    python3 serve.py [port]

Default port: 8080
"""

import http.server
import socketserver
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080

class CORSRequestHandler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        # Required for SharedArrayBuffer and multi-threaded WASM
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')

        # Additional CORS headers for development
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', '*')

        # Cache control
        self.send_header('Cache-Control', 'no-store, no-cache, must-revalidate')

        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(200)
        self.end_headers()

if __name__ == '__main__':
    with socketserver.TCPServer(("", PORT), CORSRequestHandler) as httpd:
        print(f"")
        print(f"  zafu | zcash light wallet demo")
        print(f"")
        print(f"  server running at: http://localhost:{PORT}")
        print(f"")
        print(f"  SharedArrayBuffer enabled")
        print(f"  Multi-threading enabled")
        print(f"  CORS headers configured")
        print(f"")
        print(f"  Make sure zidecar is running on port 50051")
        print(f"  Press Ctrl+C to stop")
        print(f"")

        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nServer stopped")
            sys.exit(0)
