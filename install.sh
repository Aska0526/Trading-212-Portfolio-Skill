#!/bin/sh
# T212 Portfolio installer (macOS and Linux).
#   curl -fsSL https://raw.githubusercontent.com/Aska0526/Trading-212-Portfolio-Skill/main/install.sh | sh -s -- chatgpt
#   curl -fsSL https://raw.githubusercontent.com/Aska0526/Trading-212-Portfolio-Skill/main/install.sh | sh -s -- claude-code
# Optional second argument: language (en or zh). Defaults to the system language.
#
# Downloads the release binary, verifies its SHA-256, installs it to
# ~/.local/bin, then asks for your API key in this terminal (hidden input,
# saved to the system keychain) and registers the skill.
set -eu

REPO="${T212_REPO:-Aska0526/Trading-212-Portfolio-Skill}"
VERSION="${T212_VERSION:-latest}"
CLIENT="${1:-}"
BIN_DIR="${T212_BIN_DIR:-$HOME/.local/bin}"

# Language: argument, then T212_LANG, then macOS system language, then locale.
LANG_CHOICE="${2:-${T212_LANG:-}}"
if [ -z "$LANG_CHOICE" ] && command -v defaults >/dev/null 2>&1; then
  if defaults read -g AppleLanguages 2>/dev/null | sed -n 2p | grep -qi 'zh'; then LANG_CHOICE=zh; fi
fi
if [ -z "$LANG_CHOICE" ]; then
  case "${LC_ALL:-${LANG:-}}" in zh*) LANG_CHOICE=zh ;; *) LANG_CHOICE=en ;; esac
fi
case "$LANG_CHOICE" in zh*) LANG_CHOICE=zh ;; *) LANG_CHOICE=en ;; esac
export T212_LANG="$LANG_CHOICE"

msg() { if [ "$T212_LANG" = zh ]; then printf '%s\n' "$2"; else printf '%s\n' "$1"; fi; }

case "$CLIENT" in claude-code|chatgpt) ;; *)
  msg "usage: install.sh <chatgpt|claude-code> [en|zh]" "用法：install.sh <chatgpt|claude-code> [en|zh]" >&2; exit 2 ;; esac

os=$(uname -s); arch=$(uname -m)
case "$os-$arch" in
  Darwin-arm64)  target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
  *) msg "Unsupported platform: $os $arch" "不支持的平台：$os $arch" >&2; exit 1 ;;
esac

if [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
msg "Downloading t212 ($target)..." "正在下载 t212（$target）..."
curl -fsSL "$base/t212-$target.tar.gz" -o "$tmp/t212.tar.gz"
curl -fsSL "$base/t212-$target.tar.gz.sha256" -o "$tmp/t212.sha256"

expected=$(cut -d' ' -f1 "$tmp/t212.sha256")
if command -v shasum >/dev/null 2>&1; then actual=$(shasum -a 256 "$tmp/t212.tar.gz" | cut -d' ' -f1)
else actual=$(sha256sum "$tmp/t212.tar.gz" | cut -d' ' -f1); fi
[ "$expected" = "$actual" ] || { msg "Checksum mismatch, stopping." "校验失败，已停止。" >&2; exit 1; }

mkdir -p "$BIN_DIR"
tar -xzf "$tmp/t212.tar.gz" -C "$tmp"
install -m 755 "$tmp/t212" "$BIN_DIR/t212"
msg "Installed $BIN_DIR/t212" "已安装 $BIN_DIR/t212"
echo

# stdin is the script when piped; read prompts from the terminal instead.
"$BIN_DIR/t212" setup </dev/tty
"$BIN_DIR/t212" install --client "$CLIENT" </dev/tty
case ":$PATH:" in *":$BIN_DIR:"*) ;; *)
  msg "Note: add $BIN_DIR to your PATH to use \`t212\` directly." "提示：将 $BIN_DIR 加入 PATH 后即可直接使用 \`t212\`。" ;; esac
