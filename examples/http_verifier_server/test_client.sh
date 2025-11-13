#!/bin/bash
# Test client for the HTTP verifier server

SERVER_URL="${SERVER_URL:-http://localhost:3000}"

echo "Testing HTTP Verifier Server at $SERVER_URL"
echo "============================================"
echo

# Test health endpoint
echo "1. Health Check:"
curl -s "$SERVER_URL/health" | jq .
echo
echo

# Test config endpoint
echo "2. Server Configuration:"
curl -s "$SERVER_URL/config" | jq .
echo
echo

# Test verification endpoint (example - you'd need actual proof data)
echo "3. Verification Endpoint (example):"
echo "   (This will fail without actual proof data)"
cat > /tmp/verify_request.json <<EOF
{
  "proof_size": 20,
  "proof_bytes": []
}
EOF

curl -s -X POST "$SERVER_URL/verify" \
  -H "Content-Type: application/json" \
  -d @/tmp/verify_request.json | jq .
echo
echo

echo "============================================"
echo "Test complete!"
echo
echo "To run the server:"
echo "  cargo run --manifest-path examples/http_verifier_server/Cargo.toml"
echo
echo "To test with actual proofs, use the Ligerito CLI to generate proofs first."
