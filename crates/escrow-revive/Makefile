# Escrow Revive Contract - Full Demo Makefile

-include .env
export

# Default RPC and account (override in .env)
CHAIN_RPC ?= http://127.0.0.1:9944
PRIVATE_KEY ?= 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

.PHONY: all build clean deploy demo help

all: build

# ============================================================================
# BUILD
# ============================================================================

build: build-contract build-demo

build-contract:
	@echo "Building escrow contract..."
	RUSTC_BOOTSTRAP=1 cargo build --release
	polkatool link --strip --output escrow.polkavm \
		target/riscv64emac-unknown-none-polkavm/release/escrow
	@echo "Contract size: $$(ls -lh escrow.polkavm | awk '{print $$5}')"

build-demo:
	@echo "Building demo CLI..."
	cd demo && cargo +stable build --release
	@cp demo/target/x86_64-unknown-linux-gnu/release/escrow-demo .
	@echo "Demo CLI built: ./escrow-demo"

clean:
	cargo clean
	cd demo && cargo clean
	rm -f *.polkavm escrow-demo shares.json

# ============================================================================
# DEMO FLOW
# ============================================================================

demo: build
	@echo ""
	@./escrow-demo demo

generate-shares:
	@./escrow-demo generate-shares > shares.json
	@echo "Shares saved to shares.json"

# ============================================================================
# DEPLOYMENT
# ============================================================================

deploy: build-contract
	@test -n "$(CHAIN_RPC)" || (echo "Set CHAIN_RPC in .env" && exit 1)
	@test -n "$(PRIVATE_KEY)" || (echo "Set PRIVATE_KEY in .env" && exit 1)

	@echo "Deploying to $(CHAIN_RPC)..."
	$(eval CONTRACT_ADDR := $(shell cast send --private-key $(PRIVATE_KEY) \
		--rpc-url $(CHAIN_RPC) --create \
		"0x$$(xxd -p -c 99999 escrow.polkavm)" \
		--json 2>/dev/null | jq -r .contractAddress))
	@echo "Contract deployed at: $(CONTRACT_ADDR)"

	@sed -i '/^CONTRACT_ADDRESS=/d' .env 2>/dev/null || true
	@echo "CONTRACT_ADDRESS=$(CONTRACT_ADDR)" >> .env
	@echo "Saved to .env"

# ============================================================================
# CONTRACT INTERACTION
# ============================================================================

# Create escrow: make create-escrow COMMITMENT=0x... PUBKEY=0x... SHARE_C=0x...
create-escrow:
	@test -n "$(CONTRACT_ADDRESS)" || (echo "Deploy first: make deploy" && exit 1)
	@test -n "$(COMMITMENT)" || (echo "Set COMMITMENT" && exit 1)
	@test -n "$(PUBKEY)" || (echo "Set PUBKEY (escrow pubkey)" && exit 1)
	@test -n "$(SHARE_C)" || (echo "Set SHARE_C" && exit 1)

	@echo "Creating escrow..."
	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"createEscrow(bytes32,bytes32,bytes32)" \
		$(COMMITMENT) $(PUBKEY) $(SHARE_C)

# Confirm escrow as buyer: make confirm-escrow ESCROW_ID=0x... BUYER=0x...
confirm-escrow:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)
	@test -n "$(BUYER)" || (echo "Set BUYER address" && exit 1)

	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"confirmEscrow(bytes32,address)" \
		$(ESCROW_ID) $(BUYER)

# Mark payment sent: make mark-payment ESCROW_ID=0x...
mark-payment:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)

	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"markPaymentSent(bytes32)" \
		$(ESCROW_ID)

# Confirm payment (seller): make confirm-payment ESCROW_ID=0x...
confirm-payment:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)

	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"confirmPayment(bytes32)" \
		$(ESCROW_ID)

# Raise dispute: make dispute ESCROW_ID=0x...
dispute:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)

	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"dispute(bytes32)" \
		$(ESCROW_ID)

# Resolve dispute: make resolve ESCROW_ID=0x... TO_BUYER=true/false
resolve:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)
	@test -n "$(TO_BUYER)" || (echo "Set TO_BUYER (true/false)" && exit 1)

	cast send --private-key $(PRIVATE_KEY) --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"resolveDispute(bytes32,bool)" \
		$(ESCROW_ID) $(TO_BUYER)

# Get escrow info: make get-escrow ESCROW_ID=0x...
get-escrow:
	@test -n "$(ESCROW_ID)" || (echo "Set ESCROW_ID" && exit 1)

	cast call --rpc-url $(CHAIN_RPC) \
		$(CONTRACT_ADDRESS) \
		"getEscrow(bytes32)" \
		$(ESCROW_ID)

# ============================================================================
# FULL DEMO (with local node)
# ============================================================================

full-demo: build
	@echo "╔═══════════════════════════════════════════════════════════════╗"
	@echo "║              FULL E2E ESCROW DEMO                             ║"
	@echo "╚═══════════════════════════════════════════════════════════════╝"
	@echo ""
	@echo "Prerequisites:"
	@echo "  1. Local Paseo node running with Revive pallet"
	@echo "  2. cast (foundry) installed"
	@echo "  3. Funded account"
	@echo ""
	@echo "Step 1: Generate shares..."
	@./escrow-demo demo
	@echo ""
	@echo "Step 2: Deploy contract..."
	@echo "  make deploy"
	@echo ""
	@echo "Step 3: Create escrow with generated values..."
	@echo "  make create-escrow COMMITMENT=<from above> PUBKEY=<from above> SHARE_C=<from above>"
	@echo ""
	@echo "Step 4: Continue with trade flow..."
	@echo "  make confirm-escrow ESCROW_ID=<returned> BUYER=<buyer_address>"
	@echo "  make mark-payment ESCROW_ID=<id>"
	@echo "  make confirm-payment ESCROW_ID=<id>  # Happy path"
	@echo "  OR"
	@echo "  make dispute ESCROW_ID=<id>"
	@echo "  make resolve ESCROW_ID=<id> TO_BUYER=true"

# ============================================================================
# HELP
# ============================================================================

help:
	@echo "Escrow Revive Contract"
	@echo ""
	@echo "Build:"
	@echo "  make build          - Build contract and demo CLI"
	@echo "  make clean          - Clean build artifacts"
	@echo ""
	@echo "Demo:"
	@echo "  make demo           - Run demo flow explanation"
	@echo "  make generate-shares - Generate VSS shares"
	@echo "  make full-demo      - Full E2E demo instructions"
	@echo ""
	@echo "Deploy:"
	@echo "  make deploy         - Deploy contract"
	@echo ""
	@echo "Interact:"
	@echo "  make create-escrow  - Create new escrow"
	@echo "  make confirm-escrow - Buyer confirms escrow"
	@echo "  make mark-payment   - Mark fiat payment sent"
	@echo "  make confirm-payment - Seller confirms payment received"
	@echo "  make dispute        - Raise dispute"
	@echo "  make resolve        - Resolve dispute"
	@echo "  make get-escrow     - Query escrow state"
