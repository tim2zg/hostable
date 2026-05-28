#!/usr/bin/env bash
# ==============================================================================
# Hostable LXC Image Packager
# Converts a Docker Image/Dockerfile into an LXC-compatible rootfs & metadata.
# ==============================================================================

set -euo pipefail

# Print styled messages
log_info()  { echo -e "\033[1;34m[INFO]\033[0m $1"; }
log_ok()    { echo -e "\033[1;32m[ OK ]\033[0m $1"; }
log_error() { echo -e "\033[1;31m[FAIL]\033[0m $1" >&2; }

# Help menu
show_help() {
    echo "Usage: $0 [options]"
    echo ""
    echo "Options:"
    echo "  --image   <name>    The Docker image to pull and convert (e.g. lscr.io/linuxserver/jellyfin:latest)"
    echo "  --distro  <name>    The target LXC distribution name (e.g. jellyfin)"
    echo "  --release <name>    The target release name (e.g. latest)"
    echo "  --arch    <arch>    The target architecture (default: amd64)"
    echo "  --variant <variant>  The image variant (default: default)"
    echo "  --help              Show this help message"
}

# Default values
IMAGE=""
DISTRO=""
RELEASE=""
ARCH="amd64"
VARIANT="default"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image)   IMAGE="$2"; shift 2 ;;
        --distro)  DISTRO="$2"; shift 2 ;;
        --release) RELEASE="$2"; shift 2 ;;
        --arch)    ARCH="$2"; shift 2 ;;
        --variant) VARIANT="$2"; shift 2 ;;
        --help)    show_help; exit 0 ;;
        *)         log_error "Unknown option: $1"; show_help; exit 1 ;;
    esac
done

# Validate required arguments
if [[ -z "$IMAGE" ]] || [[ -z "$DISTRO" ]] || [[ -z "$RELEASE" ]]; then
    log_error "Missing required options: --image, --distro, and --release are mandatory."
    show_help
    exit 1
fi

BUILD_DATE=$(date -u +"%Y%m%d_%H%M")
DIST_DIR="dist"
IMAGES_DIR="${DIST_DIR}/images/${DISTRO}/${RELEASE}/${ARCH}/${VARIANT}/${BUILD_DATE}"
META_DIR="${DIST_DIR}/meta/1.0"

log_info "Starting conversion for ${DISTRO}:${RELEASE} (${ARCH})"
log_info "Docker Image Source: ${IMAGE}"
log_info "Destination: ${IMAGES_DIR}"

# 1. Pull Docker image
log_info "Pulling Docker image: ${IMAGE}..."
docker pull "${IMAGE}"

# 2. Create temporary container
log_info "Creating temporary Docker container..."
CONTAINER_ID=$(docker create "${IMAGE}")
log_ok "Container created with ID: ${CONTAINER_ID:0:12}"

# Ensure container cleanup on exit or error
cleanup() {
    log_info "Cleaning up temporary resources..."
    if [[ -n "${CONTAINER_ID:-}" ]]; then
        docker rm "${CONTAINER_ID}" >/dev/null || true
    fi
    rm -rf tmp_meta_assembly rootfs.tar || true
}
trap cleanup EXIT

# 3. Export container filesystem
log_info "Exporting filesystem to rootfs.tar (this may take a minute)..."
docker export "${CONTAINER_ID}" -o rootfs.tar
log_ok "Filesystem exported successfully."

# 4. Compress rootfs.tar to rootfs.tar.xz using multi-threading
log_info "Compressing rootfs.tar to rootfs.tar.xz using xz..."
xz -T0 -6 -v rootfs.tar
log_ok "Rootfs compressed: rootfs.tar.xz"

# 5. Assemble LXC metadata (meta.tar.xz)
log_info "Assembling LXC metadata files..."
mkdir -p tmp_meta_assembly

# Create LXC container configuration file
cat <<EOF > tmp_meta_assembly/config
# Template used to create this container: hostable-lxc
# Default LXC system settings
lxc.arch = ${ARCH}
lxc.uts.name = ${DISTRO}

# System mounting behavior
lxc.mount.auto = proc:rw sys:rw
lxc.cap.drop = sys_time sys_module sys_rawio

# Standard virtual interface (will be overwritten by LXC server configurations)
lxc.net.0.type = veth
lxc.net.0.flags = up
lxc.net.0.link = lxcbr0
lxc.net.0.name = eth0
EOF

# Create container creation summary message
cat <<EOF > tmp_meta_assembly/create-message
=============================================================================
Container successfully created!
=============================================================================
* Distribution: ${DISTRO}
* Release: ${RELEASE}
* Architecture: ${ARCH}
* Packaging: Hostable LXC automated pipeline

This container is preloaded with all optimized binaries and scripts from:
  ${IMAGE}

Enjoy your high-performance container!
=============================================================================
EOF

# Create image expiration timestamp (set to 1 year from now)
date -d "+365 days" +%s > tmp_meta_assembly/expiry

# Package metadata into meta.tar.xz
log_info "Compressing metadata into meta.tar.xz..."
tar -cf - -C tmp_meta_assembly config create-message expiry | xz -T0 -6 > meta.tar.xz
log_ok "Metadata packaged successfully."

# 6. Organize into output directory structure
log_info "Creating target directories..."
mkdir -p "${IMAGES_DIR}"
mkdir -p "${META_DIR}"

log_info "Moving tarballs to release directory..."
mv rootfs.tar.xz "${IMAGES_DIR}/"
mv meta.tar.xz "${IMAGES_DIR}/"

# 7. Generate SHA-256 Checksums
log_info "Generating file integrity checksums..."
(
    cd "${IMAGES_DIR}"
    sha256sum rootfs.tar.xz > rootfs.tar.xz.sha256
    sha256sum meta.tar.xz > meta.tar.xz.sha256
)
log_ok "Checksums generated."

# 8. Update LXC index catalogs
log_info "Updating index-system and index-user catalog lists..."
INDEX_LINE="${DISTRO};${RELEASE};${ARCH};${VARIANT};${BUILD_DATE};/images/${DISTRO}/${RELEASE}/${ARCH}/${VARIANT}/${BUILD_DATE}/"

# Safely append lines to indices, preventing duplicate paths
update_index() {
    local index_file="$1"
    touch "${index_file}"
    # Remove existing entry for the exact distro/release/arch if it exists
    grep -v "^${DISTRO};${RELEASE};${ARCH};" "${index_file}" > "${index_file}.tmp" || true
    mv "${index_file}.tmp" "${index_file}"
    # Append the new build line
    echo "${INDEX_LINE}" >> "${index_file}"
}

update_index "${META_DIR}/index-system"
update_index "${META_DIR}/index-user"

log_ok "Indices updated successfully."
log_info "============================================================================="
log_ok "Successfully built LXC image for ${DISTRO}:${RELEASE}!"
log_info "============================================================================="
