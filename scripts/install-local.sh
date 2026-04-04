#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install-local.sh [options]

Install llmrouter in single-machine mode:
- server installs as a systemd service with auto-start
- server binds to 127.0.0.1:1290 by default
- TUI installs for the invoking user and is preconfigured to connect locally

Options:
  --repo owner/repo         GitHub repository, default nodca/routellm
  --tag TAG                 Release tag, default latest
  --bind ADDR               Local bind address, default 127.0.0.1:1290
  --master-key KEY          Management key, default auto-generated
  --install-dir DIR         Server install directory, default /opt/llmrouter
  --env-file PATH           Server env file, default /etc/llmrouter.env
  --config-file PATH        Server config path, default <install-dir>/llmrouter.toml
  --service-name NAME       systemd service name, default llmrouter
  --service-user USER       Linux service user, default llmrouter
  --tui-user USER           User that receives llmrouter-tui, default sudo caller
  --tui-bin-dir DIR         TUI binary dir, default <tui-user-home>/.local/bin
  --tui-config-dir DIR      TUI config dir, default <tui-user-home>/.config/llmrouter
  --skip-start              Install service but do not start it
  -h, --help                Show this help
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

raw_script_url() {
  local script_name="$1"
  local ref="main"
  if [[ "$TAG" != "latest" ]]; then
    ref="$TAG"
  fi
  echo "https://raw.githubusercontent.com/${REPO}/${ref}/scripts/${script_name}"
}

generate_master_key() {
  printf 'sk-llmrouter-%s\n' "$(head -c 18 /dev/urandom | od -An -tx1 | tr -d ' \n')"
}

resolve_user_home() {
  local user_name="$1"
  getent passwd "$user_name" | cut -d: -f6
}

REPO="${LLMROUTER_REPO:-nodca/routellm}"
TAG="${LLMROUTER_TAG:-latest}"
BIND_ADDR="${LLMROUTER_BIND_ADDR:-127.0.0.1:1290}"
MASTER_KEY="${LLMROUTER_MASTER_KEY:-}"
INSTALL_DIR="${LLMROUTER_INSTALL_DIR:-/opt/llmrouter}"
ENV_FILE="${LLMROUTER_ENV_FILE:-/etc/llmrouter.env}"
CONFIG_FILE=""
SERVICE_NAME="${LLMROUTER_SERVICE_NAME:-llmrouter}"
SERVICE_USER="${LLMROUTER_SERVICE_USER:-llmrouter}"
TUI_USER="${LLMROUTER_TUI_USER:-${SUDO_USER:-$(id -un)}}"
TUI_BIN_DIR=""
TUI_CONFIG_DIR=""
SKIP_START=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --tag) TAG="${2:-}"; shift 2 ;;
    --bind) BIND_ADDR="${2:-}"; shift 2 ;;
    --master-key) MASTER_KEY="${2:-}"; shift 2 ;;
    --install-dir) INSTALL_DIR="${2:-}"; shift 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
    --config-file) CONFIG_FILE="${2:-}"; shift 2 ;;
    --service-name) SERVICE_NAME="${2:-}"; shift 2 ;;
    --service-user) SERVICE_USER="${2:-}"; shift 2 ;;
    --tui-user) TUI_USER="${2:-}"; shift 2 ;;
    --tui-bin-dir) TUI_BIN_DIR="${2:-}"; shift 2 ;;
    --tui-config-dir) TUI_CONFIG_DIR="${2:-}"; shift 2 ;;
    --skip-start) SKIP_START=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "install-local.sh currently supports Linux only." >&2
  exit 1
fi

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Single-machine install needs root because it installs a systemd service." >&2
  echo "Run it like:" >&2
  echo "  curl -fsSL $(raw_script_url install-local.sh) | sudo bash" >&2
  exit 1
fi

require_cmd curl
require_cmd getent
require_cmd install

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
if [[ -z "$CONFIG_FILE" ]]; then
  CONFIG_FILE="${INSTALL_DIR}/llmrouter.toml"
fi
if [[ -z "$MASTER_KEY" ]]; then
  MASTER_KEY="$(generate_master_key)"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

SERVER_SCRIPT="${TMP_DIR}/install-server.sh"
TUI_SCRIPT="${TMP_DIR}/install-tui.sh"

curl -fsSL "$(raw_script_url install-server.sh)" -o "$SERVER_SCRIPT"
curl -fsSL "$(raw_script_url install-tui.sh)" -o "$TUI_SCRIPT"
chmod +x "$SERVER_SCRIPT" "$TUI_SCRIPT"

bash "$SERVER_SCRIPT" \
  --repo "$REPO" \
  --tag "$TAG" \
  --install-dir "$INSTALL_DIR" \
  --env-file "$ENV_FILE" \
  --config-file "$CONFIG_FILE" \
  --bind "$BIND_ADDR" \
  --master-key "$MASTER_KEY" \
  --service-name "$SERVICE_NAME" \
  --service-user "$SERVICE_USER" \
  $(if [[ "$SKIP_START" -eq 1 ]]; then printf '%s' '--skip-start'; fi)

mkdir -p "$TUI_BIN_DIR" "$TUI_CONFIG_DIR"
bash "$TUI_SCRIPT" \
  --repo "$REPO" \
  --tag "$TAG" \
  --bin-dir "$TUI_BIN_DIR" \
  --config-dir "$TUI_CONFIG_DIR" \
  --server "http://${BIND_ADDR}" \
  --auth-key "$MASTER_KEY"

chown -R "$TUI_USER:$TUI_USER" "$TUI_BIN_DIR" "$TUI_CONFIG_DIR"

cat <<EOF
Single-machine installation complete.

Server:
  service      ${SERVICE_NAME}
  endpoint     http://${BIND_ADDR}
  config       ${CONFIG_FILE}
  env          ${ENV_FILE}

TUI:
  user         ${TUI_USER}
  binary       ${TUI_BIN_DIR}/llmrouter-tui
  config       ${TUI_CONFIG_DIR}/tui.env

Management key:
  ${MASTER_KEY}

Next steps:
  1. Edit ${CONFIG_FILE} and add your routes/channels.
  2. Restart after editing:
     sudo systemctl restart ${SERVICE_NAME}
  3. Run TUI as ${TUI_USER}:
     ${TUI_BIN_DIR}/llmrouter-tui
EOF
