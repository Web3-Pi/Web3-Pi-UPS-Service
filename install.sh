#!/bin/bash
# Web3 Pi UPS Service Installer
# One-liner: curl -fsSL https://raw.githubusercontent.com/Web3-Pi/Web3-Pi-UPS-Service/main/install.sh | sudo bash

set -e

REPO="Web3-Pi/Web3-Pi-UPS-Service"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/w3p-ups"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_NAME="w3p-ups"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_root() {
    if [ "$EUID" -ne 0 ]; then
        log_error "This script must be run as root (use sudo)"
        exit 1
    fi
}

check_architecture() {
    ARCH=$(uname -m)
    if [ "$ARCH" != "aarch64" ]; then
        log_error "This service is designed for ARM64 (aarch64) architecture"
        log_error "Detected architecture: $ARCH"
        exit 1
    fi
}

get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/'
}

download_release() {
    local version=$1
    local url="https://github.com/${REPO}/releases/download/${version}/w3p-ups-${version}-aarch64.tar.gz"
    local tmp_dir=$(mktemp -d)

    log_info "Downloading ${SERVICE_NAME} ${version}..." >&2
    curl -fsSL "$url" -o "${tmp_dir}/release.tar.gz"

    log_info "Extracting..." >&2
    tar -xzf "${tmp_dir}/release.tar.gz" -C "${tmp_dir}"

    echo "${tmp_dir}"
}

install_binary() {
    local tmp_dir=$1

    log_info "Installing binary to ${INSTALL_DIR}..."
    install -m 755 "${tmp_dir}/w3p-ups" "${INSTALL_DIR}/w3p-ups"
}

install_config() {
    local tmp_dir=$1

    if [ ! -d "$CONFIG_DIR" ]; then
        log_info "Creating config directory ${CONFIG_DIR}..."
        mkdir -p "$CONFIG_DIR"
    fi

    if [ ! -f "${CONFIG_DIR}/config.toml" ]; then
        log_info "Installing default configuration..."
        install -m 644 "${tmp_dir}/config.toml.example" "${CONFIG_DIR}/config.toml"
    else
        log_warn "Config file already exists, skipping (backup at config.toml.example)"
        install -m 644 "${tmp_dir}/config.toml.example" "${CONFIG_DIR}/config.toml.example"
    fi

    if [ ! -f "${CONFIG_DIR}/shutdown.sh" ]; then
        log_info "Installing shutdown script..."
        install -m 755 "${tmp_dir}/shutdown.sh" "${CONFIG_DIR}/shutdown.sh"
    else
        log_warn "Shutdown script already exists, skipping"
    fi
}

install_systemd_service() {
    local tmp_dir=$1

    log_info "Installing systemd service..."
    install -m 644 "${tmp_dir}/w3p-ups.service" "${SYSTEMD_DIR}/${SERVICE_NAME}.service"

    log_info "Reloading systemd daemon..."
    systemctl daemon-reload
}

setup_permissions() {
    # Add current user to dialout group for serial port access
    if [ -n "$SUDO_USER" ]; then
        log_info "Adding user '$SUDO_USER' to dialout group for serial port access..."
        usermod -a -G dialout "$SUDO_USER" 2>/dev/null || true
    fi
}

enable_and_start_service() {
    log_info "Enabling ${SERVICE_NAME} service..."
    systemctl enable "${SERVICE_NAME}.service"

    log_info "Starting ${SERVICE_NAME} service..."
    systemctl start "${SERVICE_NAME}.service"
}

cleanup() {
    local tmp_dir=$1
    rm -rf "$tmp_dir"
}

show_status() {
    echo ""
    echo "=========================================="
    echo -e "${GREEN}Installation complete!${NC}"
    echo "=========================================="
    echo ""
    echo "Configuration file: ${CONFIG_DIR}/config.toml"
    echo "Shutdown script:    ${CONFIG_DIR}/shutdown.sh"
    echo "Binary location:    ${INSTALL_DIR}/w3p-ups"
    echo ""
    echo "Useful commands:"
    echo "  Check status:     systemctl status ${SERVICE_NAME}"
    echo "  View logs:        journalctl -u ${SERVICE_NAME} -f"
    echo "  Edit config:      nano ${CONFIG_DIR}/config.toml"
    echo "  Restart service:  systemctl restart ${SERVICE_NAME}"
    echo ""

    if systemctl is-active --quiet "${SERVICE_NAME}"; then
        log_info "Service is running"
    else
        log_warn "Service is not running. Check logs with: journalctl -u ${SERVICE_NAME} -e"
    fi
}

uninstall() {
    log_info "Stopping service..."
    systemctl stop "${SERVICE_NAME}.service" 2>/dev/null || true

    log_info "Disabling service..."
    systemctl disable "${SERVICE_NAME}.service" 2>/dev/null || true

    log_info "Removing files..."
    rm -f "${SYSTEMD_DIR}/${SERVICE_NAME}.service"
    rm -f "${INSTALL_DIR}/w3p-ups"

    log_info "Reloading systemd..."
    systemctl daemon-reload

    log_warn "Config directory ${CONFIG_DIR} was NOT removed (contains your settings)"
    log_info "To completely remove: rm -rf ${CONFIG_DIR}"

    echo ""
    log_info "Uninstallation complete!"
}

main() {
    echo ""
    echo "╔═══════════════════════════════════════════╗"
    echo "║   Web3 Pi UPS Service Installer           ║"
    echo "╚═══════════════════════════════════════════╝"
    echo ""

    # Handle uninstall
    if [ "$1" = "--uninstall" ] || [ "$1" = "-u" ]; then
        check_root
        uninstall
        exit 0
    fi

    # Handle version flag
    if [ "$1" = "--version" ] || [ "$1" = "-v" ]; then
        if [ -f "${INSTALL_DIR}/w3p-ups" ]; then
            "${INSTALL_DIR}/w3p-ups" --version
        else
            echo "w3p-ups is not installed"
        fi
        exit 0
    fi

    # Handle help
    if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
        echo "Usage: $0 [OPTIONS]"
        echo ""
        echo "Options:"
        echo "  --uninstall, -u    Uninstall the service"
        echo "  --version, -v      Show installed version"
        echo "  --help, -h         Show this help message"
        echo ""
        echo "One-liner install:"
        echo "  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sudo bash"
        exit 0
    fi

    check_root
    check_architecture

    VERSION=$(get_latest_version)
    if [ -z "$VERSION" ]; then
        log_error "Failed to get latest version from GitHub"
        exit 1
    fi

    log_info "Latest version: ${VERSION}"

    TMP_DIR=$(download_release "$VERSION")

    install_binary "$TMP_DIR"
    install_config "$TMP_DIR"
    install_systemd_service "$TMP_DIR"
    setup_permissions
    enable_and_start_service
    cleanup "$TMP_DIR"
    show_status
}

main "$@"
