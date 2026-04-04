#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
用法：install-tui.sh [选项]

选项：
  --repo owner/repo         GitHub 仓库，默认 nodca/routellm；如果传了 --asset-url 则忽略
  --tag v1.1.0              Release 标签，默认 latest
  --asset-url URL           直接指定资源下载地址
  --bin-dir DIR             二进制安装目录，默认 ~/.local/bin
  --config-dir DIR          配置目录，默认 ~/.config/llmrouter
  --server URL              TUI 连接的服务端地址，默认 http://127.0.0.1:1290
  --auth-key KEY            TUI 认证 Key，可选
  --skip-env                不写入 ~/.config/llmrouter/tui.env
  -h, --help                显示帮助
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "缺少必要命令：$1" >&2
    exit 1
  }
}

detect_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    *)
      echo "暂不支持当前操作系统：$(uname -s)" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *)
      echo "暂不支持当前架构：$(uname -m)" >&2
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

REPO="${LLMROUTER_REPO:-nodca/routellm}"
TAG="${LLMROUTER_TAG:-latest}"
ASSET_URL="${LLMROUTER_ASSET_URL:-}"
BIN_DIR="${LLMROUTER_TUI_BIN_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${LLMROUTER_TUI_CONFIG_DIR:-${HOME}/.config/llmrouter}"
SERVER_URL="${LLMROUTER_BASE_URL:-http://127.0.0.1:1290}"
AUTH_KEY="${LLMROUTER_AUTH_KEY:-}"
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
      echo "未知参数：$1" >&2
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
ASSET_NAME="llmrouter-${OS}-${ARCH}.tar.gz"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

URL="$(download_url "$ASSET_NAME")"
ARCHIVE_PATH="${TMP_DIR}/${ASSET_NAME}"
echo "正在下载：${URL}"
curl -fsSL "$URL" -o "$ARCHIVE_PATH"
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
PACKAGE_ROOT="$(find "$TMP_DIR" -mindepth 1 -maxdepth 1 -type d | head -n1)"
if [[ -z "$PACKAGE_ROOT" ]]; then
  echo "下载的压缩包中未找到程序目录" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
install -m 755 "${PACKAGE_ROOT}/llmrouter-tui" "${BIN_DIR}/llmrouter-tui"
ln -sf "${BIN_DIR}/llmrouter-tui" "${BIN_DIR}/lrtui"

ENV_FILE="${CONFIG_DIR}/tui.env"
if [[ "$SKIP_ENV" -eq 0 ]]; then
  mkdir -p "$CONFIG_DIR"
  {
    echo "LLMROUTER_BASE_URL=${SERVER_URL}"
    if [[ -n "$AUTH_KEY" ]]; then
      echo "LLMROUTER_AUTH_KEY=${AUTH_KEY}"
    fi
  } > "$ENV_FILE"
fi

cat <<EOF
TUI 安装完成。

二进制文件：
  ${BIN_DIR}/llmrouter-tui
快捷命令：
  ${BIN_DIR}/lrtui
EOF

if [[ "$SKIP_ENV" -eq 0 ]]; then
  cat <<EOF
配置文件：
  ${ENV_FILE}

启动方式：
  lrtui
EOF
else
  cat <<EOF
启动方式：
  LLMROUTER_BASE_URL=${SERVER_URL} lrtui
EOF
fi

case ":$PATH:" in
  *":${BIN_DIR}:"*) ;;
  *)
    echo
    echo "注意：${BIN_DIR} 当前不在 PATH 中。请先加入 PATH，或直接使用完整路径运行。"
    echo "可直接执行："
    echo "  ${BIN_DIR}/lrtui"
    ;;
esac
