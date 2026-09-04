#!/usr/bin/env bash
# install-corral.sh — install the prebuilt Corral release (no Rust).
#
# Resolves a GitHub Release (latest by default), verifies the release bundle
# against its published .sha256, then installs corrald as a per-user service:
# launchd on macOS (scripts/setup-corrald.sh), systemd --user on Linux
# x86_64 (scripts/setup-corrald-linux.sh — rootless, no RPM, no container;
# Bazzite-friendly). The bundle is kept under ~/.local/share/corral so the
# service keeps a stable path. User config and keys in $CORRAL_CONFIG_DIR are
# never touched.
#
# Usage:
#   bash scripts/install-corral.sh
#   bash scripts/install-corral.sh --release v0.1.0 --bind 127.0.0.1 --port 8474
#   RELEASE_URL=https://.../corral-v0.1.0-macos.tar.gz bash scripts/install-corral.sh
#   RELEASE_URL=https://.../corral-v0.1.0-linux-x86_64.tar.gz bash scripts/install-corral.sh
#   bash scripts/install-corral.sh --uninstall
#   bash scripts/install-corral.sh --self-test
#
# Env overrides: RELEASE_TAG/--release, RELEASE_URL/--url, CORRAL_RELEASE_REPO,
# CORRAL_INSTALL_DIR, CORRAL_CONFIG_DIR.
set -euo pipefail

RELEASE_TAG="${RELEASE_TAG:-}"
RELEASE_URL="${RELEASE_URL:-}"
RELEASE_REPO="${CORRAL_RELEASE_REPO:-jirathip-dev/corral}"
INSTALL_ROOT="${CORRAL_INSTALL_DIR:-$HOME/.local/share/corral}"
RELEASE_DIR="$INSTALL_ROOT/release"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
BIND="127.0.0.1"
PORT="8474"
UNINSTALL=0
SELF_TEST=0

# Release assets are per-platform and never relabeled: corral-<tag>-macos.tar.gz
# (Darwin) and corral-<tag>-linux-x86_64.tar.gz (Linux). --self-test stays
# platform-neutral; anything else refuses an unsupported platform up front.
OS_NAME="$(uname -s)"
case "$OS_NAME" in
  Darwin) PLATFORM="macos" ;;
  Linux) PLATFORM="linux-x86_64" ;;
  *) PLATFORM="unsupported" ;;
esac

usage() {
  sed -n '2,21p' "$0"
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

reject_dotdot() {
  local path="$1"
  local label="$2"
  [[ -n "$path" ]] || {
    echo "!! refusing empty $label path" >&2
    return 1
  }
  case "$path" in
    *$'\n'*|*$'\r'*)
      echo "!! refusing $label path containing a newline or carriage return: $path" >&2
      return 1
      ;;
  esac
  if [[ "$path" =~ (^|/)\.\.(/|$) ]]; then
    echo "!! refusing $label path containing a '..' component: $path" >&2
    return 1
  fi
  return 0
}

is_path_ancestor() {
  local ancestor="$1"
  local child="$2"
  [[ "$ancestor" == "/" ]] && return 0
  [[ "$child" == "$ancestor" || "$child" == "$ancestor/"* ]]
}

assert_payload_path() {
  local raw="$1"
  local label="$2"
  local path
  local home_norm
  local config_norm
  reject_dotdot "$raw" "$label" || return 1
  reject_dotdot "$HOME" "home directory" || return 1
  reject_dotdot "$CONFIG_DIR" "config directory" || return 1
  path="$(normalize_path "$raw")" || return 1
  home_norm="$(normalize_path "$HOME")" || return 1
  config_norm="$(normalize_path "$CONFIG_DIR")" || return 1
  if [[ "$path" == "/" ]] \
    || is_path_ancestor "$path" "$home_norm" \
    || is_path_ancestor "$path" "$config_norm" \
    || is_path_ancestor "$config_norm" "$path"; then
    echo "!! refusing unsafe $label path: $raw (normalized: $path)" >&2
    return 1
  fi
  case "$path" in
    /Applications)
      echo "!! refusing unsafe $label path: $path" >&2
      return 1
      ;;
  esac
  if [[ "$label" == app* && "${path##*/}" != *.app ]]; then
    echo "!! refusing $label path that does not end in .app: $path" >&2
    return 1
  fi
  return 0
}

run_self_test() {
  local bad
  local label
  local safe_root
  for bad in "$HOME/.." "$HOME/../foo" "$HOME/foo/.." "$HOME" "/" "$CONFIG_DIR"; do
    for label in "install root" "release directory"; do
      if assert_payload_path "$bad" "$label" >/dev/null 2>&1; then
        echo "!! self-test failure: accepted unsafe $label path: $bad" >&2
        return 1
      fi
    done
  done
  safe_root="${TMPDIR:-/tmp}/corral-self-test"
  if ! assert_payload_path "$safe_root/install" "install root" >/dev/null 2>&1; then
    echo "!! self-test failure: rejected safe install root path" >&2
    return 1
  fi
  if ! assert_payload_path "$safe_root/install/release" "release directory" >/dev/null 2>&1; then
    echo "!! self-test failure: rejected safe release directory path" >&2
    return 1
  fi
  echo ">> install-corral path safety self-test: PASS"
  return 0
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
    --self-test)
      SELF_TEST=1
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

if [[ "$SELF_TEST" == "1" ]]; then
  if ! run_self_test; then
    exit 1
  fi
  exit 0
fi

if [[ "$PLATFORM" == "unsupported" ]]; then
  echo "!! unsupported platform: $OS_NAME (Corral release installs support macOS and Linux x86_64)" >&2
  exit 2
fi

if [[ "$UNINSTALL" == "1" ]]; then
  assert_payload_path "$INSTALL_ROOT" "install root" || exit 1
  assert_payload_path "$RELEASE_DIR" "release directory" || exit 1
  CONFIG_DIR="$(normalize_path "$CONFIG_DIR")"
  INSTALL_ROOT="$(normalize_path "$INSTALL_ROOT")"
  RELEASE_DIR="$INSTALL_ROOT/release"

  if [[ "$PLATFORM" == "macos" ]]; then
    PLIST="$HOME/Library/LaunchAgents/com.corral.corrald.plist"
    UPDATE_PLIST="$HOME/Library/LaunchAgents/com.corral.corrald-update.plist"
    LAUNCH_UID="gui/$(id -u)"

    echo ">> Uninstalling Corral launchd agents"
    launchctl bootout "$LAUNCH_UID" "$PLIST" 2>/dev/null || true
    launchctl bootout "$LAUNCH_UID" "$UPDATE_PLIST" 2>/dev/null || true
    rm -f "$PLIST" "$UPDATE_PLIST"
  else
    # Linux: per-user systemd service (see setup-corrald-linux.sh). Unit
    # paths are derived from $HOME here so uninstall works even after the
    # release bundle is gone. Config/keys are NEVER touched.
    UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    UNIT="$UNIT_DIR/corrald.service"

    echo ">> Uninstalling corrald systemd user service"
    systemctl --user disable --now corrald.service 2>/dev/null || true
    rm -f "$UNIT"
    systemctl --user daemon-reload 2>/dev/null || true
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

assert_payload_path "$INSTALL_ROOT" "install root" || exit 1
assert_payload_path "$RELEASE_DIR" "release directory" || exit 1
INSTALL_ROOT="$(normalize_path "$INSTALL_ROOT")"
RELEASE_DIR="$INSTALL_ROOT/release"

if [[ "$PLATFORM" == "linux-x86_64" ]]; then
  # Linux installs are per-user and rootless by design (systemd --user, no
  # RPM, no container — immutable-OS friendly). Refuse root and any install
  # root outside $HOME before anything is downloaded.
  [[ "$(id -u)" -eq 0 ]] && {
    echo "!! refusing to run as root: Linux installs use a per-user systemd service; run install-corral.sh as your own user" >&2
    exit 2
  }
  machine="$(uname -m)"
  if [[ "$machine" != "x86_64" && "$machine" != "amd64" ]]; then
    echo "!! Linux release installs are x86_64 only (this host: $machine)" >&2
    exit 2
  fi
  home_norm="$(normalize_path "$HOME")" || exit 1
  if [[ "$INSTALL_ROOT" == "/" ]] || ! is_path_ancestor "$home_norm" "$INSTALL_ROOT"; then
    echo "!! Linux installs are per-user: CORRAL_INSTALL_DIR must live under \$HOME (got: $INSTALL_ROOT)" >&2
    exit 2
  fi
fi

validate_bind
validate_port
require_command curl
require_command tar
# Extraction safety: the tarball listing is checked for absolute/'..' paths
# before extraction (below), and both BSD tar (macOS) and GNU tar (Linux)
# strip leading '/' from member names by default — no per-tar option needed.
# (A former GNU-only --no-absolute-names flag was removed: GNU tar >= 1.34
# rejects that spelling, which the new ubuntu CI suite exposed.)
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
  local asset_tag
  local endpoint
  if [[ -z "$tag" || "$tag" == "latest" ]]; then
    endpoint="releases/latest"
  else
    endpoint="releases/tags/$tag"
  fi
  tag="$(gh api "repos/$RELEASE_REPO/$endpoint" --jq '.tag_name')"
  [[ -n "$tag" ]] || { echo "!! could not resolve release from $RELEASE_REPO" >&2; return 1; }

  asset_tag="${tag//\//_}"
  local bundle_name="corral-$asset_tag-$PLATFORM.tar.gz"
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

if tar -tzf "$BUNDLE_FILE" | grep -Eq '(^|/)\.\.(/|$)|(^/)'; then
  echo "!! release bundle contains an unsafe archive path; refusing to extract" >&2
  exit 1
fi

# BSD tar (macOS) and GNU tar (Linux) both strip leading '/' on extraction
# by default; the unsafe-path check above already rejected '..' and absolute
# entries, so no per-tar flag is needed.
tar -xzf "$BUNDLE_FILE" -C "$STAGE_DIR"

required=(
  "$STAGE_DIR/corrald"
  "$STAGE_DIR/scripts/setup-corrald.sh"
  "$STAGE_DIR/scripts/install-corral.sh"
  "$STAGE_DIR/scripts/update-corral.sh"
  "$STAGE_DIR/scripts/lib-corral-update-path.sh"
  "$STAGE_DIR/scripts/rotate-corral-logs.sh"
)
for path in "${required[@]}"; do
  [[ -e "$path" ]] || { echo "!! release bundle is missing $path" >&2; exit 1; }
done
if [[ "$PLATFORM" == "linux-x86_64" ]]; then
  [[ -e "$STAGE_DIR/scripts/setup-corrald-linux.sh" ]] || {
    echo "!! release bundle is missing scripts/setup-corrald-linux.sh" >&2
    exit 1
  }
fi
[[ -x "$STAGE_DIR/corrald" ]] || {
  echo "!! release bundle daemon binary is not executable" >&2
  exit 1
}
if [[ "$PLATFORM" == "linux-x86_64" ]]; then
  # Never relabel a macOS artifact: the staged binary must be a Linux ELF.
  magic="$(head -c 4 "$STAGE_DIR/corrald" 2>/dev/null || true)"
  if [[ "$magic" != $'\x7fELF' ]]; then
    echo "!! staged corrald is not a Linux ELF binary (corral-*-linux-x86_64.tar.gz must contain the Linux build) — refusing to install" >&2
    exit 1
  fi
fi

# Linux update semantics: the systemd unit's ExecStart path (RELEASE_DIR/
# corrald) does not move between releases, so a binary swap alone must not
# restart the running service. Tell the setup helper whether the binary at
# that path actually changed since the service last started (installer knows:
# it is about to swap the release directory). Equal hash -> idempotent
# reinstall, no restart. Fresh install -> nothing to compare -> "no".
BINARY_CHANGED="no"
if [[ "$PLATFORM" == "linux-x86_64" && -e "$RELEASE_DIR/corrald" ]]; then
  old_hash="$(sha256_of "$RELEASE_DIR/corrald")"
  new_hash="$(sha256_of "$STAGE_DIR/corrald")"
  if [[ -n "$old_hash" && -n "$new_hash" && "$old_hash" != "$new_hash" ]]; then
    BINARY_CHANGED="yes"
  fi
fi

if [[ -e "$RELEASE_DIR" || -L "$RELEASE_DIR" ]]; then
  if [[ -e "$RELEASE_DIR.previous" || -L "$RELEASE_DIR.previous" ]]; then
    rm -rf -- "$RELEASE_DIR.previous"
  fi
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

echo ">> Installing prebuilt corrald"
if [[ "$PLATFORM" == "linux-x86_64" ]]; then
  setup_ok=0
  if bash "$RELEASE_DIR/scripts/setup-corrald-linux.sh" \
    --from-release "$RELEASE_DIR/corrald" \
    --bind "$BIND" \
    --port "$PORT" \
    --changed "$BINARY_CHANGED"; then
    setup_ok=1
  fi
else
  setup_ok=0
  if bash "$RELEASE_DIR/scripts/setup-corrald.sh" \
    --from-release "$RELEASE_DIR/corrald" \
    --bind "$BIND" \
    --port "$PORT"; then
    setup_ok=1
  fi
fi
if [[ "$setup_ok" != "1" ]]; then
  echo "!! setup failed; restoring previous release" >&2
  rm -rf -- "$RELEASE_DIR"
  if [[ -e "$RELEASE_DIR.previous" || -L "$RELEASE_DIR.previous" ]]; then
    if ! mv -- "$RELEASE_DIR.previous" "$RELEASE_DIR"; then
      echo "!! could not restore previous release; it remains at $RELEASE_DIR.previous" >&2
      exit 1
    fi
    echo "   restored $RELEASE_DIR.previous -> $RELEASE_DIR"
  fi
  exit 1
fi
rm -rf -- "$RELEASE_DIR.previous"

echo
echo ">> Installed Corral $ASSET_NAME"
if [[ "$PLATFORM" == "macos" ]]; then
  echo "   daemon:  launchctl print gui/$(id -u)/com.corral.corrald"
else
  echo "   service: systemctl --user status corrald (logs: journalctl --user -u corrald)"
fi
echo "   config:  $CONFIG_DIR"
echo "   re-run:  $0 --uninstall"
