#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
用法：install-server.sh [选项]

选项：
  --repo owner/repo         GitHub 仓库，默认 nodca/routellm；如果传了 --asset-url 则忽略
  --tag v1.1.0              Release 标签，默认 latest
  --asset-url URL           直接指定资源下载地址
  --install-dir DIR         安装目录，默认 /opt/llmrouter
  --bin-path PATH           二进制安装路径，默认 <install-dir>/llmrouter
  --config-file PATH        配置文件路径，默认 <install-dir>/llmrouter.toml
  --env-file PATH           环境变量文件路径，默认 /etc/llmrouter.env
  --bind ADDR               监听地址，默认 0.0.0.0:1290
  --master-key KEY          下游认证 Key，默认自动生成
  --request-timeout SECS    请求超时秒数，默认 90
  --service-name NAME       systemd 服务名，默认 llmrouter
  --service-user USER       Linux 服务用户，默认 llmrouter
  --skip-systemd            不安装 systemd 服务单元
  --skip-start              不启用或启动服务
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

generate_master_key() {
  printf 'sk-llmrouter-%s\n' "$(head -c 18 /dev/urandom | od -An -tx1 | tr -d ' \n')"
}

read_env_value() {
  local file="$1"
  local key="$2"
  [[ -f "$file" ]] || return 1
  awk -F= -v wanted="$key" '$1 == wanted {print substr($0, index($0, "=") + 1); exit}' "$file"
}

prepare_path_parent() {
  local path="$1"
  local parent
  parent="$(dirname "$path")"
  mkdir -p "$parent" 2>/dev/null || {
    echo "无法创建目录：$parent" >&2
    echo "请尝试用 sudo 运行，或传入 --install-dir \$HOME/.local/share/llmrouter 做当前用户安装。" >&2
    exit 1
  }
}

REPO="${LLMROUTER_REPO:-nodca/routellm}"
TAG="${LLMROUTER_TAG:-latest}"
ASSET_URL="${LLMROUTER_ASSET_URL:-}"
INSTALL_DIR="${LLMROUTER_INSTALL_DIR:-/opt/llmrouter}"
BIN_PATH=""
CONFIG_FILE=""
ENV_FILE="${LLMROUTER_ENV_FILE:-/etc/llmrouter.env}"
BIND_ADDR="${LLMROUTER_BIND_ADDR:-}"
MASTER_KEY="${LLMROUTER_MASTER_KEY:-}"
REQUEST_TIMEOUT="${LLMROUTER_REQUEST_TIMEOUT_SECS:-}"
DATABASE_URL=""
SERVICE_NAME="${LLMROUTER_SERVICE_NAME:-llmrouter}"
SERVICE_USER="${LLMROUTER_SERVICE_USER:-llmrouter}"
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
      echo "未知参数：$1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

OS="$(detect_os)"
ARCH="$(detect_arch)"
ASSET_NAME="llmrouter-${OS}-${ARCH}.tar.gz"

if [[ -z "$BIN_PATH" ]]; then
  BIN_PATH="${INSTALL_DIR}/llmrouter"
fi
if [[ -z "$CONFIG_FILE" ]]; then
  CONFIG_FILE="${INSTALL_DIR}/llmrouter.toml"
fi
if [[ -z "$BIND_ADDR" ]]; then
  BIND_ADDR="$(read_env_value "$ENV_FILE" "LLMROUTER_BIND_ADDR" || true)"
fi
if [[ -z "$BIND_ADDR" ]]; then
  BIND_ADDR="0.0.0.0:1290"
fi
if [[ -z "$REQUEST_TIMEOUT" ]]; then
  REQUEST_TIMEOUT="$(read_env_value "$ENV_FILE" "LLMROUTER_REQUEST_TIMEOUT_SECS" || true)"
fi
if [[ -z "$REQUEST_TIMEOUT" ]]; then
  REQUEST_TIMEOUT="90"
fi
if [[ -z "$MASTER_KEY" ]]; then
  MASTER_KEY="$(read_env_value "$ENV_FILE" "LLMROUTER_MASTER_KEY" || true)"
fi
if [[ -z "$MASTER_KEY" ]]; then
  MASTER_KEY="$(generate_master_key)"
fi
DATABASE_URL="$(read_env_value "$ENV_FILE" "LLMROUTER_DATABASE_URL" || true)"
if [[ -z "$DATABASE_URL" ]]; then
  DATABASE_URL="sqlite://${INSTALL_DIR}/llmrouter-state.db"
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
echo "正在下载：${URL}"
curl -fsSL "$URL" -o "$ARCHIVE_PATH"
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
PACKAGE_ROOT="$(find "$TMP_DIR" -mindepth 1 -maxdepth 1 -type d | head -n1)"
if [[ -z "$PACKAGE_ROOT" ]]; then
  echo "下载的压缩包中未找到程序目录" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
install -m 755 "${PACKAGE_ROOT}/llmrouter" "$BIN_PATH"

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
LLMROUTER_BIND_ADDR=${BIND_ADDR}
LLMROUTER_DATABASE_URL=${DATABASE_URL}
LLMROUTER_REQUEST_TIMEOUT_SECS=${REQUEST_TIMEOUT}
LLMROUTER_MASTER_KEY=${MASTER_KEY}
LLMROUTER_CONFIG_PATH=${CONFIG_FILE}
EOF

if [[ "$SKIP_SYSTEMD" -eq 0 ]]; then
  if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
    echo "安装 systemd 服务需要 root 权限" >&2
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
Description=llmrouter lightweight LLM router
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
服务端安装完成。

二进制文件：
  ${BIN_PATH}

配置文件：
  ${CONFIG_FILE}

环境文件：
  ${ENV_FILE}

管理 Key：
  ${MASTER_KEY}

后续步骤：
  1. 编辑 ${CONFIG_FILE}，添加你的 route 和 channel；也可以稍后通过 API/TUI 配置。
  2. 如果跳过了 systemd，请手动启动：
     set -a; . "${ENV_FILE}"; set +a; "${BIN_PATH}"
  3. 如果你想做非 root 安装，可以这样重装：
     --install-dir "$HOME/.local/share/llmrouter" --env-file "$HOME/.config/llmrouter/server.env" --skip-systemd
  4. 如果使用默认的 /opt 安装目录，请用 sudo 运行安装脚本。
EOF
