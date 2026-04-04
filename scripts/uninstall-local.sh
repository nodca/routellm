#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: uninstall-local.sh [options]

Remove a standard single-machine llmrouter installation from Linux:
- stops and disables the systemd service
- removes server files and env file
- removes the local TUI binary and saved connection config

Options:
  --service-name NAME       systemd service name, default llmrouter
  --service-user USER       Linux service user, default llmrouter
  --keep-service-user       Do not delete the Linux service user
  --install-dir DIR         Server install directory, default /opt/llmrouter
  --env-file PATH           Server env file, default /etc/llmrouter.env
  --tui-user USER           User that owns llmrouter-tui, default sudo caller
  --tui-bin-dir DIR         TUI binary dir, default <tui-user-home>/.local/bin
  --tui-config-dir DIR      TUI config dir, default <tui-user-home>/.config/llmrouter
  -h, --help                Show this help
EOF
}

resolve_user_home() {
  local user_name="$1"
  getent passwd "$user_name" | cut -d: -f6
}

SERVICE_NAME="${LLMROUTER_SERVICE_NAME:-llmrouter}"
SERVICE_USER="${LLMROUTER_SERVICE_USER:-llmrouter}"
KEEP_SERVICE_USER=0
INSTALL_DIR="${LLMROUTER_INSTALL_DIR:-/opt/llmrouter}"
ENV_FILE="${LLMROUTER_ENV_FILE:-/etc/llmrouter.env}"
TUI_USER="${LLMROUTER_TUI_USER:-${SUDO_USER:-$(id -un)}}"
TUI_BIN_DIR=""
TUI_CONFIG_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --service-name) SERVICE_NAME="${2:-}"; shift 2 ;;
    --service-user) SERVICE_USER="${2:-}"; shift 2 ;;
    --keep-service-user) KEEP_SERVICE_USER=1; shift ;;
    --install-dir) INSTALL_DIR="${2:-}"; shift 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
    --tui-user) TUI_USER="${2:-}"; shift 2 ;;
    --tui-bin-dir) TUI_BIN_DIR="${2:-}"; shift 2 ;;
    --tui-config-dir) TUI_CONFIG_DIR="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "uninstall-local.sh currently supports Linux only." >&2
  exit 1
fi

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Single-machine uninstall needs root because it removes the systemd service." >&2
  exit 1
fi

if ! id -u "$TUI_USER" >/dev/null 2>&1; then
  echo "TUI user not found: $TUI_USER" >&2
  exit 1
fi

TUI_HOME="$(resolve_user_home "$TUI_USER")"
if [[ -z "$TUI_HOME" ]]; then
  echo "Could not resolve home directory for user: $TUI_USER" >&2
  exit 1
fi

if [[ -z "$TUI_BIN_DIR" ]]; then
  TUI_BIN_DIR="${TUI_HOME}/.local/bin"
fi
if [[ -z "$TUI_CONFIG_DIR" ]]; then
  TUI_CONFIG_DIR="${TUI_HOME}/.config/llmrouter"
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl stop "${SERVICE_NAME}" 2>/dev/null || true
  systemctl disable "${SERVICE_NAME}" 2>/dev/null || true
  rm -f "/etc/systemd/system/${SERVICE_NAME}.service"
  systemctl daemon-reload 2>/dev/null || true
fi

rm -rf "$INSTALL_DIR"
rm -f "$ENV_FILE"

rm -f "${TUI_BIN_DIR}/llmrouter-tui" "${TUI_BIN_DIR}/lrtui"
rm -rf "$TUI_CONFIG_DIR"

if [[ "$KEEP_SERVICE_USER" -eq 0 ]] && id -u "$SERVICE_USER" >/dev/null 2>&1; then
  userdel "$SERVICE_USER" 2>/dev/null || true
fi

cat <<EOF
Single-machine uninstall complete.

Removed:
  service      ${SERVICE_NAME}
  install dir  ${INSTALL_DIR}
  env file     ${ENV_FILE}
  TUI binary   ${TUI_BIN_DIR}/llmrouter-tui
  TUI alias    ${TUI_BIN_DIR}/lrtui
  TUI config   ${TUI_CONFIG_DIR}
EOF
