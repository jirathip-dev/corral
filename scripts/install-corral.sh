#!/usr/bin/env bash
# install-corral.sh — install the prebuilt macOS Corral release (no Rust).
#
# Resolves a GitHub Release (latest by default), verifies the release bundle
# against its published .sha256, then installs corrald under launchd and the
# egui board through scripts/setup-corrald.sh --from-release. The bundle is
# kept under ~/.local/share/corral so the launchd/update agents keep stable
# paths. User config and keys in $CORRAL_CONFIG_DIR are never touched.
#
# Usage:
#   bash scripts/install-corral.sh
#   bash scripts/install-corral.sh --release v0.1.0 --bind 127.0.0.1 --port 8474
#   RELEASE_URL=https://.../corral-v0.1.0-macos.tar.gz bash scripts/install-corral.sh
#   bash scripts/install-corral.sh --uninstall
#
# Env overrides: RELEASE_TAG/--release, RELEASE_URL/--url, CORRAL_RELEASE_REPO,
# CORRAL_INSTALL_DIR, CORRAL_CONFIG_DIR, CORRAL_MACOS_APP_DEST.
set -euo pipefail

RELEASE_TAG="${RELEASE_TAG:-}"
RELEASE_URL="${RELEASE_URL:-}"
RELEASE_REPO="${CORRAL_RELEASE_REPO:-jirathip-dev/corral}"
INSTALL_ROOT="${CORRAL_INSTALL_DIR:-$HOME/.local/share/corral}"
RELEASE_DIR="$INSTALL_ROOT/release"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
APP_DEST="${CORRAL_MACOS_APP_DEST:-/Applications/Corral.app}"
BIND="127.0.0.1"
PORT="8474"
UNINSTALL=0

usage() {
  sed -n '2,14p' "$0"
}

normalize_path() {
  local input="$1"
  local -a parts=()
  local -a stack=()
  local component
  [[ "$input" == /* ]] || input="$(pwd -P)/$input"
  IFS='/' read -r -a parts <<< "$input"
  for component in "${parts[@]}"; do
    case "$component" in
      ""|".") ;;
      "..")
        if [[ "${#stack[@]}" -eq 0 ]]; then
          echo "!! refusing path that escapes an absolute root: $input" >&2
          return 1
        fi
        unset "stack[${#stack[@]}-1]"
        ;;
      *) stack+=("$component") ;;
    esac
  done
  if [[ "${#stack[@]}" -eq 0 ]]; then
    printf '/'
  else
    printf '/%s' "$(IFS=/; echo "${stack[*]}")"
  fi
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "!! required command not found: $1" >&2
    exit 1
  }
}

validate_bind() {
  if ! [[ "$BIND" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] && ! [[ "$BIND" =~ ^[0-9a-fA-F:]+$ ]]; then
    echo "!! invalid --bind address (IPv4 or IPv6 only): $BIND" >&2
    exit 2
  fi
}

validate_port() {
  if ! [[ "$PORT" =~ ^[0-9]+$ ]] || (( PORT < 1 || PORT > 65535 )); then
    echo "!! invalid --port (1-65535): $PORT" >&2
    exit 2
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      [[ $# -ge 2 && -n "$2" ]] || { echo "!! --release requires a tag" >&2; exit 2; }
      RELEASE_TAG="$2"
      shift 2
      ;;
    --url)
      [[ $# -ge 2 && -n "$2" ]] || { echo "!! --url requires a URL" >&2; exit 2; }
      RELEASE_URL="$2"
      shift 2
      ;;
    --bind)
      [[ $# -ge 2 && -n "$2" ]] || { echo "!! --bind requires an address" >&2; exit 2; }
      BIND="$2"
      shift 2
      ;;
    --port)
      [[ $# -ge 2 && -n "$2" ]] || { echo "!! --port requires a value" >&2; exit 2; }
      PORT="$2"
      shift 2
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

[[ -n "$RELEASE_TAG" && -n "$RELEASE_URL" ]] && {
  echo "!! --release and --url are mutually exclusive" >&2
  exit 2
}

if [[ "$UNINSTALL" == "1" ]]; then
  PLIST="$HOME/Library/LaunchAgents/com.corral.corrald.plist"
  UPDATE_PLIST="$HOME/Library/LaunchAgents/com.corral.corrald-update.plist"
  LAUNCH_UID="gui/$(id -u)"
  CONFIG_DIR="$(normalize_path "$CONFIG_DIR")"
  INSTALL_ROOT="$(normalize_path "$INSTALL_ROOT")"
  RELEASE_DIR="$INSTALL_ROOT/release"

  case "$APP_DEST" in
    *$'\n'*|*$'\r'*|*'/../'*)
      echo "!! refusing unsafe app uninstall path: $APP_DEST" >&2
      exit 1
      ;;
    /*) ;;
    *) APP_DEST="$(pwd -P)/$APP_DEST" ;;
  esac
  APP_DEST="$(normalize_path "$APP_DEST")"
  if [[ "$APP_DEST" == "/" || "$APP_DEST" == "/Applications" || "$APP_DEST" == "$HOME" || "$APP_DEST" == "$CONFIG_DIR" || "$APP_DEST" == "$CONFIG_DIR/"* ]]; then
    echo "!! refusing to uninstall unsafe app path: $APP_DEST" >&2
    exit 1
  fi
  if [[ "$RELEASE_DIR" == "/" || "$RELEASE_DIR" == "$HOME" || "$RELEASE_DIR" == "$CONFIG_DIR" || "$RELEASE_DIR" == "$CONFIG_DIR/"* ]]; then
    echo "!! refusing to uninstall unsafe release path: $RELEASE_DIR" >&2
    exit 1
  fi

  echo ">> Uninstalling Corral launchd agents"
  launchctl bootout "$LAUNCH_UID" "$PLIST" 2>/dev/null || true
  launchctl bootout "$LAUNCH_UID" "$UPDATE_PLIST" 2>/dev/null || true
  rm -f "$PLIST" "$UPDATE_PLIST"
  if [[ -d "$APP_DEST" || -L "$APP_DEST" ]]; then
    echo ">> Removing $APP_DEST"
    pkill -f "$APP_DEST/Contents/MacOS/corrald-ui" 2>/dev/null || true
    rm -rf -- "$APP_DEST"
  fi

  if [[ -e "$RELEASE_DIR" || -L "$RELEASE_DIR" ]]; then
    echo ">> Removing downloaded release files: $RELEASE_DIR"
    rm -rf -- "$RELEASE_DIR"
  fi
  if [[ -e "$RELEASE_DIR.previous" || -L "$RELEASE_DIR.previous" ]]; then
    echo ">> Removing previous release backup: $RELEASE_DIR.previous"
    rm -rf -- "$RELEASE_DIR.previous"
  fi
  echo ">> Uninstall complete. Config/keys kept at $CONFIG_DIR."
  exit 0
fi

validate_bind
validate_port
require_command curl
require_command tar
if command -v shasum >/dev/null 2>&1; then
  CHECKSUM_CMD="shasum -a 256"
elif command -v sha256sum >/dev/null 2>&1; then
  CHECKSUM_CMD="sha256sum"
elif command -v openssl >/dev/null 2>&1; then
  CHECKSUM_CMD="openssl dgst -sha256"
else
  echo "!! no SHA-256 tool found (need shasum, sha256sum, or openssl)" >&2
  exit 1
fi

sha256_of() {
  case "$CHECKSUM_CMD" in
    openssl*) $CHECKSUM_CMD "$1" | awk '{print $NF}' ;;
    *) $CHECKSUM_CMD "$1" | awk '{print $1}' ;;
  esac
}

resolve_release_urls() {
  local tag="$RELEASE_TAG"
  local endpoint
  if [[ -z "$tag" || "$tag" == "latest" ]]; then
    endpoint="releases/latest"
  else
    endpoint="releases/tags/$tag"
  fi
  tag="$(gh api "repos/$RELEASE_REPO/$endpoint" --jq '.tag_name')"
  [[ -n "$tag" ]] || { echo "!! could not resolve release from $RELEASE_REPO" >&2; return 1; }

  local bundle_name="corral-$tag-macos.tar.gz"
  local asset_list
  asset_list="$(gh api "repos/$RELEASE_REPO/releases/tags/$tag" --jq '.assets[] | [.name, .browser_download_url] | @tsv')"
  ASSET_URL="$(awk -F '\t' -v name="$bundle_name" '$1 == name {print $2; exit}' <<<"$asset_list")"
  CHECKSUM_URL="$(awk -F '\t' -v name="$bundle_name.sha256" '$1 == name {print $2; exit}' <<<"$asset_list")"
  [[ -n "$ASSET_URL" && -n "$CHECKSUM_URL" ]] || {
    echo "!! release $tag is missing $bundle_name or $bundle_name.sha256" >&2
    return 1
  }
  ASSET_NAME="$bundle_name"
  printf 'Using release %s (%s)\n' "$tag" "$bundle_name"
}

if [[ -n "$RELEASE_URL" ]]; then
  ASSET_URL="$RELEASE_URL"
  ASSET_NAME="$(basename "$RELEASE_URL")"
  CHECKSUM_URL="$RELEASE_URL.sha256"
else
  require_command gh
  resolve_release_urls
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/corral-install.XXXXXX")"
trap 'rm -rf -- "$WORK_DIR"' EXIT

BUNDLE_FILE="$WORK_DIR/$ASSET_NAME"
CHECKSUM_FILE="$WORK_DIR/$ASSET_NAME.sha256"
echo ">> Downloading $ASSET_NAME"
curl -fsSL "$ASSET_URL" -o "$BUNDLE_FILE"
curl -fsSL "$CHECKSUM_URL" -o "$CHECKSUM_FILE"
[[ -s "$BUNDLE_FILE" ]] || { echo "!! downloaded bundle is empty" >&2; exit 1; }

expected="$(tr -d '\r\n' < "$CHECKSUM_FILE")"
if ! [[ "$expected" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "!! published checksum is malformed: $CHECKSUM_URL" >&2
  exit 1
fi
actual="$(sha256_of "$BUNDLE_FILE")"
if [[ "$actual" != "$expected" ]]; then
  echo "!! SHA-256 mismatch — refusing to install" >&2
  echo "   expected: $expected" >&2
  echo "   actual:   $actual" >&2
  exit 1
fi
echo ">> SHA-256 verified: $actual"

mkdir -p "$INSTALL_ROOT"
STAGE_DIR="$(mktemp -d "$INSTALL_ROOT/.release-stage.XXXXXX")"
STAGE_CLEANED=0
cleanup_stage() {
  if [[ "$STAGE_CLEANED" == "0" && -n "${STAGE_DIR:-}" && -e "$STAGE_DIR" ]]; then
    rm -rf -- "$STAGE_DIR"
  fi
}
trap 'cleanup_stage; rm -rf -- "$WORK_DIR"' EXIT

tar -xzf "$BUNDLE_FILE" -C "$STAGE_DIR"

required=(
  "$STAGE_DIR/corrald"
  "$STAGE_DIR/corrald-ui"
  "$STAGE_DIR/scripts/setup-corrald.sh"
  "$STAGE_DIR/scripts/install-corral-ui.sh"
  "$STAGE_DIR/scripts/update-corral.sh"
  "$STAGE_DIR/assets/icon/corral-icon-macos.png"
  "$STAGE_DIR/assets/icon/corral-icon-256.png"
)
for path in "${required[@]}"; do
  [[ -e "$path" ]] || { echo "!! release bundle is missing $path" >&2; exit 1; }
done
[[ -x "$STAGE_DIR/corrald" && -x "$STAGE_DIR/corrald-ui" ]] || {
  echo "!! release bundle binaries are not executable" >&2
  exit 1
}

rm -rf -- "$RELEASE_DIR.previous"
if [[ -e "$RELEASE_DIR" || -L "$RELEASE_DIR" ]]; then
  mv -- "$RELEASE_DIR" "$RELEASE_DIR.previous"
fi
if ! mv -- "$STAGE_DIR" "$RELEASE_DIR"; then
  if [[ -e "$RELEASE_DIR.previous" ]]; then
    mv -- "$RELEASE_DIR.previous" "$RELEASE_DIR" 2>/dev/null || true
  fi
  echo "!! could not move verified release into $RELEASE_DIR" >&2
  exit 1
fi
STAGE_CLEANED=1
rm -rf -- "$RELEASE_DIR.previous"

echo ">> Installing prebuilt corrald + egui board"
bash "$RELEASE_DIR/scripts/setup-corrald.sh" \
  --from-release "$RELEASE_DIR/corrald" \
  --bind "$BIND" \
  --port "$PORT"

echo
echo ">> Installed Corral $ASSET_NAME"
echo "   daemon:  launchctl print gui/$(id -u)/com.corral.corrald"
echo "   board:   $APP_DEST"
echo "   config:  $CONFIG_DIR"
echo "   re-run:  $0 --uninstall"
