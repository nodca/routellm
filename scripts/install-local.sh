#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
用法：install-local.sh [选项]

单机模式安装 llmrouter：
- server 以 systemd 服务形式安装，并默认开机自启
- server 默认监听 127.0.0.1:1290
- TUI 安装到当前用户环境，并预先配置为连接本机服务端

选项：
  --repo owner/repo         GitHub 仓库，默认 nodca/routellm
  --tag TAG                 Release 标签，默认 latest
  --bind ADDR               本机监听地址，默认 127.0.0.1:1290
  --master-key KEY          管理 Key，默认自动生成
  --install-dir DIR         服务端安装目录，默认 /opt/llmrouter
  --env-file PATH           服务端环境文件，默认 /etc/llmrouter.env
  --config-file PATH        服务端配置文件，默认 <install-dir>/llmrouter.toml
  --service-name NAME       systemd 服务名，默认 llmrouter
  --service-user USER       Linux 服务用户，默认 llmrouter
  --tui-user USER           接收 llmrouter-tui 的用户，默认 sudo 调用者
  --tui-bin-dir DIR         TUI 二进制目录，默认 <tui-user-home>/.local/bin
  --tui-config-dir DIR      TUI 配置目录，默认 <tui-user-home>/.config/llmrouter
  --skip-start              安装服务但不立即启动
  -h, --help                显示帮助
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "缺少必要命令：$1" >&2
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
      echo "未知参数：$1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "install-local.sh 当前仅支持 Linux。" >&2
  exit 1
fi

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "单机安装需要 root 权限，因为它会安装 systemd 服务。" >&2
  echo "建议这样运行：" >&2
  echo "  curl -fsSL $(raw_script_url install-local.sh) | sudo bash" >&2
  exit 1
fi

require_cmd curl
require_cmd getent
require_cmd install

if ! id -u "$TUI_USER" >/dev/null 2>&1; then
  echo "未找到 TUI 用户：$TUI_USER" >&2
  exit 1
fi

TUI_HOME="$(resolve_user_home "$TUI_USER")"
if [[ -z "$TUI_HOME" ]]; then
  echo "无法解析用户家目录：$TUI_USER" >&2
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
单机安装完成。

服务端：
  service      ${SERVICE_NAME}
  endpoint     http://${BIND_ADDR}
  config       ${CONFIG_FILE}
  env          ${ENV_FILE}

TUI：
  user         ${TUI_USER}
  binary       ${TUI_BIN_DIR}/llmrouter-tui
  alias        ${TUI_BIN_DIR}/lrtui
  config       ${TUI_CONFIG_DIR}/tui.env

管理 Key：
  ${MASTER_KEY}

后续步骤：
  1. 编辑 ${CONFIG_FILE}，添加你的 route 和 channel。
  2. 修改配置后重启服务：
     sudo systemctl restart ${SERVICE_NAME}
  3. 以 ${TUI_USER} 身份运行 TUI：
     lrtui
EOF
