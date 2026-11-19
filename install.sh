#!/usr/bin/env bash
set -euo pipefail

REPO="${CODEXRAY_REPO:-jaon13/code-xray}"
INSTALL_DIR="${CODEXRAY_HOME:-$HOME/.agents/code-xray}"
FROM_SOURCE="${CODEXRAY_FROM_SOURCE:-0}"

for arg in "$@"; do
  case "$arg" in
    --from-source) FROM_SOURCE=1 ;;
    *) ;;
  esac
done

PY_BIN="$(command -v python3 || command -v python || true)"
if [[ -z "$PY_BIN" ]]; then
  echo "Error: Python 3.10+ is required." >&2
  exit 1
fi

OS_RAW="$(uname -s)"
ARCH_RAW="$(uname -m)"
case "$OS_RAW" in
  Linux*) OS_TAG="linux" ;;
  Darwin*) OS_TAG="darwin" ;;
  *) OS_TAG="$(echo "$OS_RAW" | tr '[:upper:]' '[:lower:]')" ;;
esac
case "$ARCH_RAW" in
  x86_64|amd64) ARCH_TAG="amd64" ;;
  arm64|aarch64) ARCH_TAG="arm64" ;;
  *) ARCH_TAG="$ARCH_RAW" ;;
esac

ASSET_NAME="CodeXray-${OS_TAG}-${ARCH_TAG}.zip"
INSTALLED_FROM_RELEASE=0

mkdir -p "$INSTALL_DIR"

if [[ "$FROM_SOURCE" != "1" ]] && command -v curl >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  DL_URL="$("$PY_BIN" -c '
import json, sys, urllib.request
repo, asset = sys.argv[1], sys.argv[2]
req = urllib.request.Request(
    f"https://api.github.com/repos/{repo}/releases/latest",
    headers={"User-Agent": "code-xray-installer"},
)
try:
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    for a in data.get("assets", []):
        if a.get("name") == asset:
            print(a.get("browser_download_url", ""))
            break
except Exception:
    pass
' "$REPO" "$ASSET_NAME")"
  if [[ -n "$DL_URL" ]]; then
    TMP_ZIP="$(mktemp /tmp/code-xray-XXXXXX.zip)"
    echo "[code-xray] Downloading prebuilt release ($ASSET_NAME) ..."
    curl -fsSL "$DL_URL" -o "$TMP_ZIP"
    unzip -qo "$TMP_ZIP" -d "$INSTALL_DIR"
    rm -f "$TMP_ZIP"
    "$PY_BIN" "$INSTALL_DIR/scripts/publish_tools.py" --skip-rust --skip-dotnet --register-mcp --install-wrappers
    INSTALLED_FROM_RELEASE=1
  fi
fi

if [[ "$INSTALLED_FROM_RELEASE" != "1" ]]; then
  if command -v git >/dev/null 2>&1; then
    if [[ -d "$INSTALL_DIR/.git" ]]; then
      echo "[code-xray] Updating $INSTALL_DIR ..."
      git -C "$INSTALL_DIR" pull --ff-only
    else
      rm -rf "$INSTALL_DIR"
      echo "[code-xray] Cloning https://github.com/$REPO.git -> $INSTALL_DIR ..."
      git clone --depth 1 "https://github.com/$REPO.git" "$INSTALL_DIR"
    fi
  else
    TMP_ZIP="$(mktemp /tmp/code-xray-src-XXXXXX.zip)"
    TMP_DIR="$(mktemp -d /tmp/code-xray-src-XXXXXX)"
    curl -fsSL "https://github.com/$REPO/archive/refs/heads/main.zip" -o "$TMP_ZIP"
    unzip -qo "$TMP_ZIP" -d "$TMP_DIR"
    cp -R "$TMP_DIR"/*/* "$INSTALL_DIR/"
    rm -rf "$TMP_ZIP" "$TMP_DIR"
  fi

  if ! command -v cargo >/dev/null 2>&1 || ! command -v dotnet >/dev/null 2>&1; then
    echo "Error: Building from source requires Rust (cargo) and .NET 10 SDK (dotnet)." >&2
    exit 1
  fi
  "$PY_BIN" "$INSTALL_DIR/scripts/publish_tools.py" --skip-tests --register-mcp --install-wrappers
fi

echo "[code-xray] Installed at $INSTALL_DIR"
echo "[code-xray] Restart Antigravity / Cursor to reload the 'code-xray' MCP server."
