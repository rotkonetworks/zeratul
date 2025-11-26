#!/bin/bash
# ligerito release helper
# usage:
#   ./scripts/ligerito-release.sh status            # show current state
#   ./scripts/ligerito-release.sh version 0.2.3     # set version on all crates
#   ./scripts/ligerito-release.sh local             # enable local paths (dev mode)
#   ./scripts/ligerito-release.sh publish           # disable local paths (for publishing)
#   ./scripts/ligerito-release.sh release 0.2.3    # full release: set version + publish all

set -e
cd "$(dirname "$0")/.."

CRATES_DIR="crates"

# auto-discover ligerito crates in dependency order
get_crates() {
    # order matters for publishing: deps first
    echo "ligerito-binary-fields"
    echo "ligerito-merkle"
    echo "ligerito-reed-solomon"
    echo "ligerito"
}

# find all ligerito-* Cargo.toml files
get_cargo_files() {
    find "$CRATES_DIR" -maxdepth 2 -name "Cargo.toml" -path "*ligerito*" | sort
}

cmd_version() {
    local ver=$1
    if [[ -z "$ver" ]]; then
        echo "usage: $0 version <version>"
        exit 1
    fi

    echo "setting version to $ver..."

    for crate in $(get_crates); do
        if [[ -f "$CRATES_DIR/$crate/Cargo.toml" ]]; then
            sed -i "0,/^version = \".*\"/s//version = \"$ver\"/" "$CRATES_DIR/$crate/Cargo.toml"
            echo "  $crate -> $ver"
        fi
    done

    # update cross-dependencies in all ligerito Cargo.toml files
    for f in $(get_cargo_files); do
        sed -i "s/ligerito-binary-fields = { version = \"[^\"]*\"/ligerito-binary-fields = { version = \"$ver\"/g" "$f"
        sed -i "s/ligerito-merkle = { version = \"[^\"]*\"/ligerito-merkle = { version = \"$ver\"/g" "$f"
        sed -i "s/ligerito-reed-solomon = { version = \"[^\"]*\"/ligerito-reed-solomon = { version = \"$ver\"/g" "$f"
    done

    echo "done"
}

cmd_local() {
    echo "enabling local paths (dev mode)..."

    for f in $(get_cargo_files); do
        # add path for each dep if not already present
        sed -i 's|ligerito-binary-fields = { version = "\([^"]*\)"|ligerito-binary-fields = { version = "\1", path = "../ligerito-binary-fields"|g' "$f"
        sed -i 's|ligerito-merkle = { version = "\([^"]*\)"|ligerito-merkle = { version = "\1", path = "../ligerito-merkle"|g' "$f"
        sed -i 's|ligerito-reed-solomon = { version = "\([^"]*\)"|ligerito-reed-solomon = { version = "\1", path = "../ligerito-reed-solomon"|g' "$f"
    done

    echo "done - local paths enabled"
}

cmd_publish() {
    echo "disabling local paths (publish mode)..."

    for f in $(get_cargo_files); do
        sed -i 's|, path = "\.\./ligerito-binary-fields"||g' "$f"
        sed -i 's|, path = "\.\./ligerito-merkle"||g' "$f"
        sed -i 's|, path = "\.\./ligerito-reed-solomon"||g' "$f"
    done

    echo "done - ready for publishing"
}

cmd_release() {
    local ver=$1
    if [[ -z "$ver" ]]; then
        echo "usage: $0 release <version>"
        exit 1
    fi

    cmd_version "$ver"
    cmd_publish

    echo ""
    echo "publishing crates in order..."

    for crate in $(get_crates); do
        if [[ -f "$CRATES_DIR/$crate/Cargo.toml" ]]; then
            echo "publishing $crate v$ver..."
            cargo publish -p "$crate" --no-verify --allow-dirty
            echo "  waiting for crates.io index..."
            sleep 5
        fi
    done

    echo ""
    echo "restoring local paths..."
    cmd_local

    echo ""
    echo "release $ver complete!"
}

cmd_status() {
    echo "current versions:"
    for crate in $(get_crates); do
        if [[ -f "$CRATES_DIR/$crate/Cargo.toml" ]]; then
            ver=$(grep "^version" "$CRATES_DIR/$crate/Cargo.toml" | head -1 | cut -d'"' -f2)
            echo "  $crate: $ver"
        fi
    done

    echo ""
    echo "local paths:"
    if grep -q 'path = "\.\./ligerito' "$CRATES_DIR/ligerito/Cargo.toml" 2>/dev/null; then
        echo "  enabled (dev mode)"
    else
        echo "  disabled (publish mode)"
    fi
}

case "$1" in
    version) cmd_version "$2" ;;
    local)   cmd_local ;;
    publish) cmd_publish ;;
    release) cmd_release "$2" ;;
    status)  cmd_status ;;
    *)
        echo "ligerito release helper"
        echo ""
        echo "usage:"
        echo "  $0 status              # show current state"
        echo "  $0 version <ver>       # set version on all crates"
        echo "  $0 local               # enable local paths (dev)"
        echo "  $0 publish             # disable local paths"
        echo "  $0 release <ver>       # full release workflow"
        ;;
esac
