#!/usr/bin/env bash
# Toggle ligerito dependencies between dev mode (with path) and publish mode (version only)
#
# Usage:
#   ./scripts/toggle-deps.sh dev      # Enable path dependencies for local development
#   ./scripts/toggle-deps.sh publish  # Remove path dependencies for crates.io publishing
#   ./scripts/toggle-deps.sh status   # Show current mode

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATES_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

# Crates that have internal dependencies
CRATES=(
    "ligerito-reed-solomon"
    "ligerito"
)

# Pattern for dependencies with path
# Matches: { version = "X.Y.Z", path = "../something", ... }
# or:      { version = "X.Y.Z", path = "../something" }
PATH_PATTERN='(ligerito-[a-z-]+) = \{ version = "([^"]+)", path = "[^"]+"(, [^}]+)? \}'
VERSION_ONLY_PATTERN='(ligerito-[a-z-]+) = \{ version = "([^"]+)"(, [^}]+)? \}'

to_publish_mode() {
    echo "Switching to PUBLISH mode (removing path dependencies)..."

    for crate in "${CRATES[@]}"; do
        cargo_toml="$CRATES_DIR/$crate/Cargo.toml"
        if [[ -f "$cargo_toml" ]]; then
            echo "  Processing $crate..."
            # Remove path = "..." from dependency specs
            sed -i -E 's/(ligerito-[a-z-]+ = \{ version = "[^"]+"), path = "[^"]+"([,}])/\1\2/g' "$cargo_toml"
        fi
    done

    echo "Done! Dependencies are now in publish mode."
    echo "Run 'cargo publish --dry-run' to verify."
}

to_dev_mode() {
    echo "Switching to DEV mode (adding path dependencies)..."

    # ligerito-reed-solomon depends on ligerito-binary-fields
    cargo_toml="$CRATES_DIR/ligerito-reed-solomon/Cargo.toml"
    if [[ -f "$cargo_toml" ]]; then
        echo "  Processing ligerito-reed-solomon..."
        sed -i -E 's/(ligerito-binary-fields = \{ version = "[^"]+")([,}])/\1, path = "..\/ligerito-binary-fields"\2/g' "$cargo_toml"
        # Clean up double path if already present
        sed -i -E 's/, path = "[^"]+", path = "[^"]+"/,  path = "..\/ligerito-binary-fields"/g' "$cargo_toml"
    fi

    # ligerito depends on all three
    cargo_toml="$CRATES_DIR/ligerito/Cargo.toml"
    if [[ -f "$cargo_toml" ]]; then
        echo "  Processing ligerito..."
        sed -i -E 's/(ligerito-binary-fields = \{ version = "[^"]+")([,}])/\1, path = "..\/ligerito-binary-fields"\2/g' "$cargo_toml"
        sed -i -E 's/(ligerito-merkle = \{ version = "[^"]+")([,}])/\1, path = "..\/ligerito-merkle"\2/g' "$cargo_toml"
        sed -i -E 's/(ligerito-reed-solomon = \{ version = "[^"]+")([,}])/\1, path = "..\/ligerito-reed-solomon"\2/g' "$cargo_toml"
        # Clean up double path if already present
        sed -i -E 's/, path = "[^"]+", path = "[^"]+"/,  path = "..\/ligerito-binary-fields"/g' "$cargo_toml"
    fi

    echo "Done! Dependencies are now in dev mode."
}

show_status() {
    echo "Current dependency status:"
    echo
    for crate in "${CRATES[@]}"; do
        cargo_toml="$CRATES_DIR/$crate/Cargo.toml"
        if [[ -f "$cargo_toml" ]]; then
            echo "[$crate]"
            grep "^ligerito-" "$cargo_toml" | while read -r line; do
                if echo "$line" | grep -q 'path = '; then
                    echo "  DEV:     $line"
                else
                    echo "  PUBLISH: $line"
                fi
            done
            echo
        fi
    done
}

case "${1:-status}" in
    dev|development|local)
        to_dev_mode
        ;;
    publish|release|prod)
        to_publish_mode
        ;;
    status|show)
        show_status
        ;;
    *)
        echo "Usage: $0 {dev|publish|status}"
        echo
        echo "  dev     - Enable path dependencies for local development"
        echo "  publish - Remove path dependencies for crates.io publishing"
        echo "  status  - Show current mode for each dependency"
        exit 1
        ;;
esac
