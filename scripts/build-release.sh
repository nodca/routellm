#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/build-release.sh [--tag v1.1.0] [--output-dir dist]

Build release binaries for the current host and package them into a GitHub Releases asset.
EOF
}

TAG=""
OUTPUT_DIR="dist"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      TAG="${2:-}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file"
  else
    shasum -a 256 "$file"
  fi
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

require_cmd cargo
require_cmd tar
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
  echo "Missing required command: sha256sum or shasum" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OS="$(detect_os)"
ARCH="$(detect_arch)"
ASSET_BASENAME="llmrouter-${OS}-${ARCH}"

mkdir -p "${ROOT_DIR}/${OUTPUT_DIR}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

pushd "$ROOT_DIR" >/dev/null
cargo build --release --bin llmrouter --bin llmrouter-tui
popd >/dev/null

PACKAGE_ROOT="${TMP_DIR}/${ASSET_BASENAME}"
mkdir -p "${PACKAGE_ROOT}/examples"

install -m 755 "${ROOT_DIR}/target/release/llmrouter" "${PACKAGE_ROOT}/llmrouter"
install -m 755 "${ROOT_DIR}/target/release/llmrouter-tui" "${PACKAGE_ROOT}/llmrouter-tui"
install -m 644 "${ROOT_DIR}/examples/llmrouter.service" "${PACKAGE_ROOT}/examples/llmrouter.service"
install -m 644 "${ROOT_DIR}/examples/llmrouter.toml" "${PACKAGE_ROOT}/examples/llmrouter.toml"
install -m 644 "${ROOT_DIR}/README.md" "${PACKAGE_ROOT}/README.md"

ARCHIVE_PATH="${ROOT_DIR}/${OUTPUT_DIR}/${ASSET_BASENAME}.tar.gz"
tar -C "$TMP_DIR" -czf "$ARCHIVE_PATH" "$ASSET_BASENAME"

pushd "${ROOT_DIR}/${OUTPUT_DIR}" >/dev/null
sha256_file "$(basename "$ARCHIVE_PATH")" > SHA256SUMS
popd >/dev/null

cat <<EOF
Built release asset:
  ${ARCHIVE_PATH}

Release tag:
  ${TAG:-<not set>}

Checksums:
  ${ROOT_DIR}/${OUTPUT_DIR}/SHA256SUMS
EOF
