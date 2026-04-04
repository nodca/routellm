#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install-tui.sh [options]

Options:
  --repo owner/repo         GitHub repository, default nodca/routellm unless --asset-url is set
  --tag v0.2.2              Release tag, defaults to latest
  --asset-url URL           Direct asset URL override
  --bin-dir DIR             Binary install directory, default ~/.local/bin
  --config-dir DIR          Config directory, default ~/.config/routellm
  --server URL              TUI target server, default http://127.0.0.1:8080
  --auth-key KEY            TUI auth key, optional
  --skip-env                Do not write ~/.config/routellm/tui.env
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

REPO="${METAPI_REPO:-nodca/routellm}"
TAG="${METAPI_TAG:-latest}"
ASSET_URL="${METAPI_ASSET_URL:-}"
BIN_DIR="${METAPI_TUI_BIN_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${METAPI_TUI_CONFIG_DIR:-${HOME}/.config/routellm}"
SERVER_URL="${METAPI_BASE_URL:-http://127.0.0.1:8080}"
AUTH_KEY="${METAPI_AUTH_KEY:-}"
SKIP_ENV=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --tag) TAG="${2:-}"; shift 2 ;;
    --asset-url) ASSET_URL="${2:-}"; shift 2 ;;
    --bin-dir) BIN_DIR="${2:-}"; shift 2 ;;
    --config-dir) CONFIG_DIR="${2:-}"; shift 2 ;;
    --server) SERVER_URL="${2:-}"; shift 2 ;;
    --auth-key) AUTH_KEY="${2:-}"; shift 2 ;;
    --skip-env) SKIP_ENV=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

require_cmd curl
require_cmd tar
require_cmd install

OS="$(detect_os)"
ARCH="$(detect_arch)"
ASSET_NAME="metapi-${OS}-${ARCH}.tar.gz"

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

mkdir -p "$BIN_DIR"
install -m 755 "${PACKAGE_ROOT}/metapi-tui" "${BIN_DIR}/metapi-tui"

ENV_FILE="${CONFIG_DIR}/tui.env"
if [[ "$SKIP_ENV" -eq 0 ]]; then
  mkdir -p "$CONFIG_DIR"
  {
    echo "METAPI_BASE_URL=${SERVER_URL}"
    if [[ -n "$AUTH_KEY" ]]; then
      echo "METAPI_AUTH_KEY=${AUTH_KEY}"
    fi
  } > "$ENV_FILE"
fi

cat <<EOF
TUI installation complete.

Binary:
  ${BIN_DIR}/metapi-tui
EOF

if [[ "$SKIP_ENV" -eq 0 ]]; then
  cat <<EOF
Env file:
  ${ENV_FILE}

Run:
  set -a; . "${ENV_FILE}"; set +a; "${BIN_DIR}/metapi-tui"
EOF
else
  cat <<EOF
Run:
  METAPI_BASE_URL=${SERVER_URL} ${BIN_DIR}/metapi-tui
EOF
fi

case ":$PATH:" in
  *":${BIN_DIR}:"*) ;;
  *)
    echo
    echo "Note: ${BIN_DIR} is not in your PATH. Add it or run the binary with its full path."
    ;;
esac
