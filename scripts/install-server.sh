#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install-server.sh [options]

Options:
  --repo owner/repo         GitHub repository, default nodca/routellm unless --asset-url is set
  --tag v0.2.0              Release tag, defaults to latest
  --asset-url URL           Direct asset URL override
  --install-dir DIR         Install directory, default /opt/metapi
  --bin-path PATH           Binary install path, default <install-dir>/metapi-rs
  --config-file PATH        Config path, default <install-dir>/metapi.toml
  --env-file PATH           Environment file, default /etc/metapi.env
  --bind ADDR               Bind address, default 0.0.0.0:8080
  --master-key KEY          Downstream auth key, default auto-generated
  --request-timeout SECS    Request timeout, default 90
  --service-name NAME       systemd service name, default metapi
  --service-user USER       Linux service user, default metapi
  --skip-systemd            Do not install a systemd unit
  --skip-start              Do not enable/start the service
  -h, --help                Show this help
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

detect_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    *)
      echo "Unsupported OS: $(uname -s)" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *)
      echo "Unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

download_url() {
  local asset="$1"
  if [[ -n "$ASSET_URL" ]]; then
    echo "$ASSET_URL"
    return
  fi

  if [[ "$TAG" == "latest" ]]; then
    echo "https://github.com/${REPO}/releases/latest/download/${asset}"
  else
    echo "https://github.com/${REPO}/releases/download/${TAG}/${asset}"
  fi
}

generate_master_key() {
  printf 'sk-metapi-%s\n' "$(head -c 18 /dev/urandom | od -An -tx1 | tr -d ' \n')"
}

prepare_path_parent() {
  local path="$1"
  local parent
  parent="$(dirname "$path")"
  mkdir -p "$parent" 2>/dev/null || {
    echo "Failed to prepare directory: $parent" >&2
    echo "Try running as root or choose a writable path." >&2
    exit 1
  }
}

REPO="${METAPI_REPO:-nodca/routellm}"
TAG="${METAPI_TAG:-latest}"
ASSET_URL="${METAPI_ASSET_URL:-}"
INSTALL_DIR="${METAPI_INSTALL_DIR:-/opt/metapi}"
BIN_PATH=""
CONFIG_FILE=""
ENV_FILE="${METAPI_ENV_FILE:-/etc/metapi.env}"
BIND_ADDR="${METAPI_BIND_ADDR:-0.0.0.0:8080}"
MASTER_KEY="${METAPI_MASTER_KEY:-}"
REQUEST_TIMEOUT="${METAPI_REQUEST_TIMEOUT_SECS:-90}"
SERVICE_NAME="${METAPI_SERVICE_NAME:-metapi}"
SERVICE_USER="${METAPI_SERVICE_USER:-metapi}"
SKIP_SYSTEMD=0
SKIP_START=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --tag) TAG="${2:-}"; shift 2 ;;
    --asset-url) ASSET_URL="${2:-}"; shift 2 ;;
    --install-dir) INSTALL_DIR="${2:-}"; shift 2 ;;
    --bin-path) BIN_PATH="${2:-}"; shift 2 ;;
    --config-file) CONFIG_FILE="${2:-}"; shift 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
    --bind) BIND_ADDR="${2:-}"; shift 2 ;;
    --master-key) MASTER_KEY="${2:-}"; shift 2 ;;
    --request-timeout) REQUEST_TIMEOUT="${2:-}"; shift 2 ;;
    --service-name) SERVICE_NAME="${2:-}"; shift 2 ;;
    --service-user) SERVICE_USER="${2:-}"; shift 2 ;;
    --skip-systemd) SKIP_SYSTEMD=1; shift ;;
    --skip-start) SKIP_START=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

OS="$(detect_os)"
ARCH="$(detect_arch)"
ASSET_NAME="metapi-${OS}-${ARCH}.tar.gz"

if [[ -z "$BIN_PATH" ]]; then
  BIN_PATH="${INSTALL_DIR}/metapi-rs"
fi
if [[ -z "$CONFIG_FILE" ]]; then
  CONFIG_FILE="${INSTALL_DIR}/metapi.toml"
fi
DATABASE_URL="sqlite://${INSTALL_DIR}/metapi-state.db"
if [[ -z "$MASTER_KEY" ]]; then
  MASTER_KEY="$(generate_master_key)"
fi

require_cmd curl
require_cmd tar
require_cmd install

prepare_path_parent "$BIN_PATH"
prepare_path_parent "$CONFIG_FILE"
prepare_path_parent "$ENV_FILE"

if [[ "$OS" != "linux" ]]; then
  SKIP_SYSTEMD=1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

URL="$(download_url "$ASSET_NAME")"
ARCHIVE_PATH="${TMP_DIR}/${ASSET_NAME}"
echo "Downloading ${URL}"
curl -fsSL "$URL" -o "$ARCHIVE_PATH"
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
PACKAGE_ROOT="$(find "$TMP_DIR" -mindepth 1 -maxdepth 1 -type d | head -n1)"
if [[ -z "$PACKAGE_ROOT" ]]; then
  echo "Downloaded archive does not contain a package directory" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
install -m 755 "${PACKAGE_ROOT}/metapi-rs" "$BIN_PATH"

if [[ ! -f "$CONFIG_FILE" ]]; then
  cat > "$CONFIG_FILE" <<EOF
[routing]
default_cooldown_seconds = 300

[routing.cooldowns]
auth_error = 1800
rate_limited = 45
upstream_server_error = 300
transport_error = 30
edge_blocked = 1800
upstream_path_error = 1800
unknown_error = 300

[routing.manual_intervention]
auth_error = true
upstream_path_error = true
EOF
fi

cat > "$ENV_FILE" <<EOF
METAPI_BIND_ADDR=${BIND_ADDR}
METAPI_DATABASE_URL=${DATABASE_URL}
METAPI_REQUEST_TIMEOUT_SECS=${REQUEST_TIMEOUT}
METAPI_MASTER_KEY=${MASTER_KEY}
METAPI_CONFIG_PATH=${CONFIG_FILE}
EOF

if [[ "$SKIP_SYSTEMD" -eq 0 ]]; then
  if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
    echo "systemd installation requires root" >&2
    exit 1
  fi

  if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
    useradd --system --home-dir "$INSTALL_DIR" --shell /usr/sbin/nologin "$SERVICE_USER" 2>/dev/null \
      || useradd --system --home-dir "$INSTALL_DIR" --shell /sbin/nologin "$SERVICE_USER"
  fi
  chown -R "$SERVICE_USER:$SERVICE_USER" "$INSTALL_DIR"

  SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
  cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=metapi-rs lightweight LLM router
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_USER}
WorkingDirectory=${INSTALL_DIR}
EnvironmentFile=${ENV_FILE}
ExecStart=${BIN_PATH}
Restart=always
RestartSec=3
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  if [[ "$SKIP_START" -eq 0 ]]; then
    systemctl enable --now "${SERVICE_NAME}.service"
  fi
fi

cat <<EOF
Server installation complete.

Binary:
  ${BIN_PATH}

Config:
  ${CONFIG_FILE}

Env:
  ${ENV_FILE}

Master key:
  ${MASTER_KEY}

Next steps:
  1. Edit ${CONFIG_FILE} and add your routes/channels, or onboard them later via API/TUI.
  2. If systemd was skipped, start manually:
     set -a; . "${ENV_FILE}"; set +a; "${BIN_PATH}"
EOF
