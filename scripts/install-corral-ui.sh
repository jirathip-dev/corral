#!/usr/bin/env bash
# install-corral-ui.sh — stage and transactionally install the Corral desktop UI
#
# The staging directory is created beside the destination so every final move
# stays on one filesystem. Existing payloads are moved to a rollback directory
# before the staged payload is renamed into place; any failed move restores the
# prior payload.
set -euo pipefail

usage() {
  sed -n '2,8p' "$0"
}

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI_BIN="$REPO_DIR/target/release/corrald-ui"
INSTALL_PLATFORM="${CORRAL_INSTALL_PLATFORM:-$(uname -s)}"
MACOS_APP_DEST="${CORRAL_MACOS_APP_DEST:-/Applications/Corral.app}"
LINUX_PREFIX="${CORRAL_LINUX_PREFIX:-$HOME/.local}"
OTHER_PREFIX="${CORRAL_OTHER_PREFIX:-$HOME/.local}"
MACOS_ICON="$REPO_DIR/assets/icon/corral-icon-macos.png"
LINUX_ICON="$REPO_DIR/assets/icon/corral-icon-256.png"
SKIP_CODESIGN="${CORRAL_SKIP_CODESIGN:-0}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      [[ $# -ge 2 ]] || { echo "!! --binary requires a path" >&2; exit 2; }
      UI_BIN="$2"
      shift 2
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

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "!! required command not found: $1" >&2
    return 1
  }
}

require_binary() {
  [[ -f "$UI_BIN" && -x "$UI_BIN" ]] || {
    echo "!! corrald-ui binary is missing or not executable: $UI_BIN" >&2
    return 1
  }
}

rollback_directory() {
  local destination="$1"
  local rollback_dir="$2"
  local had_destination="$3"
  local rollback_ok=0

  if [[ -e "$destination" || -L "$destination" ]]; then
    if ! rm -rf "$destination"; then
      rollback_ok=1
    fi
  fi
  if [[ "$rollback_ok" == "0" && "$had_destination" == "1" ]]; then
    if ! mv "$rollback_dir/previous" "$destination"; then
      rollback_ok=1
    fi
  fi
  if [[ "$rollback_ok" == "0" ]]; then
    if ! rm -rf "$rollback_dir"; then
      rollback_ok=1
    fi
  fi
  if [[ "$rollback_ok" != "0" ]]; then
    echo "!! rollback failed; prior app is retained in rollback directory: $rollback_dir" >&2
  fi
  return "$rollback_ok"
}

commit_directory() {
  local stage="$1"
  local destination="$2"
  local parent
  parent="$(dirname "$destination")"
  local rollback_dir
  rollback_dir="$(mktemp -d "$parent/.corral-ui.rollback.XXXXXX")"
  local had_destination=0

  if [[ -e "$destination" || -L "$destination" ]]; then
    if ! mv "$destination" "$rollback_dir/previous"; then
      rm -rf "$rollback_dir"
      echo "!! could not move the existing desktop app into rollback storage" >&2
      return 1
    fi
    had_destination=1
  fi

  if ! mv "$stage" "$destination"; then
    if ! rollback_directory "$destination" "$rollback_dir" "$had_destination"; then
      return 1
    fi
    echo "!! could not install the staged desktop app; existing app restored" >&2
    return 1
  fi

  if ! rm -rf "$rollback_dir"; then
    echo "!! installed Corral.app; rollback copy retained at $rollback_dir" >&2
  fi
}

commit_files() {
  local stage="$1"
  local prefix="$2"
  shift 2
  local -a relative_paths=("$@")
  local rollback_dir
  rollback_dir="$(mktemp -d "$prefix/.corral-ui.rollback.XXXXXX")"
  local -a backed_up=()
  local -a installed=()

  rollback_files() {
    local rollback_ok=0
    local index
    local relative
    local target
    local backup

    for ((index = ${#installed[@]} - 1; index >= 0; index--)); do
      relative="${installed[$index]}"
      target="$prefix/$relative"
      if [[ -e "$target" || -L "$target" ]]; then
        if ! rm -rf "$target"; then
          rollback_ok=1
          continue
        fi
      fi
    done
    for ((index = ${#backed_up[@]} - 1; index >= 0; index--)); do
      relative="${backed_up[$index]}"
      target="$prefix/$relative"
      backup="$rollback_dir/$relative"
      if ! mkdir -p "$(dirname "$target")"; then
        rollback_ok=1
        continue
      fi
      if [[ ! -e "$backup" && ! -L "$backup" ]]; then
        rollback_ok=1
        continue
      fi
      if ! mv "$backup" "$target"; then
        rollback_ok=1
      fi
    done
    if [[ "$rollback_ok" == "0" ]]; then
      if ! rm -rf "$rollback_dir"; then
        rollback_ok=1
      fi
    fi
    if [[ "$rollback_ok" != "0" ]]; then
      echo "!! rollback failed; prior Linux payload is retained in rollback directory: $rollback_dir" >&2
    fi
    return "$rollback_ok"
  }

  local relative
  local target
  local backup
  for relative in "${relative_paths[@]}"; do
    target="$prefix/$relative"
    if [[ -e "$target" || -L "$target" ]]; then
      backup="$rollback_dir/$relative"
      mkdir -p "$(dirname "$backup")"
      if ! mv "$target" "$backup"; then
        rollback_files || true
        echo "!! could not move an existing Linux payload into rollback storage" >&2
        return 1
      fi
      backed_up+=("$relative")
    fi
  done

  for relative in "${relative_paths[@]}"; do
    target="$prefix/$relative"
    mkdir -p "$(dirname "$target")"
    if ! mv "$stage/$relative" "$target"; then
      if ! rollback_files; then
        return 1
      fi
      echo "!! could not install the staged Linux payload; existing payload restored" >&2
      return 1
    fi
    installed+=("$relative")
  done

  if ! rm -rf "$rollback_dir"; then
    echo "!! installed the Linux payload; rollback copies retained at $rollback_dir" >&2
  fi
}

install_macos() (
  set -euo pipefail
  local app_parent
  app_parent="$(dirname "$MACOS_APP_DEST")"
  [[ "$MACOS_APP_DEST" != "/" && -n "$MACOS_APP_DEST" ]] || {
    echo "!! refusing an unsafe macOS app destination: $MACOS_APP_DEST" >&2
    exit 1
  }
  mkdir -p "$app_parent"
  require_binary
  [[ -f "$MACOS_ICON" ]] || {
    echo "!! macOS icon source missing: $MACOS_ICON" >&2
    exit 1
  }
  require_command sips
  require_command iconutil
  require_command plutil
  if [[ "$SKIP_CODESIGN" != "1" ]]; then
    require_command codesign
  fi

  local stage
  local iconset
  local icon_verify
  stage="$(mktemp -d "$app_parent/.corral-ui.stage.XXXXXX")"
  iconset="$stage/Contents/Resources/Corral.iconset"
  icon_verify=""
  cleanup_macos() {
    if [[ -n "$stage" && -e "$stage" ]]; then rm -rf "$stage"; fi
    if [[ -n "$icon_verify" && -e "$icon_verify" ]]; then rm -rf "$icon_verify"; fi
  }
  trap cleanup_macos EXIT

  mkdir -p "$stage/Contents/MacOS" "$iconset"
  cp "$UI_BIN" "$stage/Contents/MacOS/corrald-ui"
  chmod +x "$stage/Contents/MacOS/corrald-ui"

  local spec
  local pixels
  local name
  for spec in "16:16x16" "32:16x16@2x" "32:32x32" "64:32x32@2x" "128:128x128" "256:128x128@2x" "256:256x256" "512:256x256@2x" "512:512x512" "1024:512x512@2x"; do
    pixels="${spec%%:*}"
    name="${spec##*:}"
    sips -z "$pixels" "$pixels" "$MACOS_ICON" --out "$iconset/icon_$name.png" >/dev/null
    [[ -s "$iconset/icon_$name.png" ]] || {
      echo "!! sips produced an empty macOS icon slice: $name" >&2
      exit 1
    }
  done
  iconutil -c icns "$iconset" -o "$stage/Contents/Resources/Corral.icns"
  [[ -s "$stage/Contents/Resources/Corral.icns" ]] || {
    echo "!! iconutil produced an empty Corral.icns" >&2
    exit 1
  }

  icon_verify="$(mktemp -d "$app_parent/.corral-ui.icns-check.XXXXXX")"
  iconutil -c iconset "$stage/Contents/Resources/Corral.icns" -o "$icon_verify/Corral.iconset"
  [[ -s "$icon_verify/Corral.iconset/icon_16x16.png" ]] || {
    echo "!! generated Corral.icns failed iconset validation" >&2
    exit 1
  }
  rm -rf "$iconset"

  cat > "$stage/Contents/Info.plist" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>corrald-ui</string>
  <key>CFBundleIdentifier</key><string>com.corral.corrald-ui</string>
  <key>CFBundleName</key><string>Corral</string>
  <key>CFBundleDisplayName</key><string>Corral</string>
  <key>CFBundleIconFile</key><string>Corral</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST_EOF
  plutil -lint "$stage/Contents/Info.plist" >/dev/null
  grep -Fq '<key>CFBundleIconFile</key><string>Corral</string>' "$stage/Contents/Info.plist"
  [[ -x "$stage/Contents/MacOS/corrald-ui" ]]

  if [[ "$SKIP_CODESIGN" != "1" ]]; then
    codesign -s - --force "$stage"
    codesign --verify --deep --strict "$stage"
  fi

  rm -rf "$icon_verify"
  icon_verify=""
  commit_directory "$stage" "$MACOS_APP_DEST"
  echo "   ✓ installed. Launch: open $MACOS_APP_DEST"
)

install_linux() (
  set -euo pipefail
  [[ "$LINUX_PREFIX" != "/" && -n "$LINUX_PREFIX" ]] || {
    echo "!! refusing an unsafe Linux install prefix: $LINUX_PREFIX" >&2
    exit 1
  }
  mkdir -p "$LINUX_PREFIX"
  require_binary
  [[ -f "$LINUX_ICON" ]] || {
    echo "!! Linux desktop icon missing: $LINUX_ICON" >&2
    exit 1
  }

  desktop_exec_quote() {
    local value="$1"
    local output='"'
    local character
    while [[ -n "$value" ]]; do
      character="${value%"${value#?}"}"
      value="${value#?}"
      case "$character" in
        \\) output+='\\' ;;
        \") output+='\"' ;;
        %) output+='%%' ;;
        $'\n'|$'\r')
          echo "!! Linux executable path contains a desktop-entry newline" >&2
          exit 1
          ;;
        *) output+="$character" ;;
      esac
    done
    output+='"'
    printf '%s' "$output"
  }

  local stage
  stage="$(mktemp -d "$LINUX_PREFIX/.corral-ui.stage.XXXXXX")"
  cleanup_linux() {
    if [[ -n "$stage" && -e "$stage" ]]; then rm -rf "$stage"; fi
  }
  trap cleanup_linux EXIT

  mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/256x256/apps"
  cp "$UI_BIN" "$stage/bin/corrald-ui"
  chmod +x "$stage/bin/corrald-ui"
  cp "$LINUX_ICON" "$stage/share/icons/hicolor/256x256/apps/corral.png"
  local desktop_exec
  desktop_exec="$(desktop_exec_quote "$LINUX_PREFIX/bin/corrald-ui")"
  cat > "$stage/share/applications/corral.desktop" <<DESKTOP_EOF
[Desktop Entry]
Type=Application
Name=Corral
Comment=Corral agent-fleet board
Exec=$desktop_exec
Icon=corral
Terminal=false
Categories=Development;
DESKTOP_EOF

  [[ -x "$stage/bin/corrald-ui" ]]
  [[ -s "$stage/share/icons/hicolor/256x256/apps/corral.png" ]]
  grep -Fqx "Exec=$desktop_exec" "$stage/share/applications/corral.desktop"
  grep -Fqx 'Icon=corral' "$stage/share/applications/corral.desktop"

  commit_files "$stage" "$LINUX_PREFIX" \
    bin/corrald-ui \
    share/icons/hicolor/256x256/apps/corral.png \
    share/applications/corral.desktop
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$LINUX_PREFIX/share/applications" 2>/dev/null || true
  fi
  echo "   ✓ installed. Launch: $LINUX_PREFIX/bin/corrald-ui"
)

install_other() (
  set -euo pipefail
  local prefix="$OTHER_PREFIX"
  [[ "$prefix" != "/" && -n "$prefix" ]] || {
    echo "!! refusing an unsafe install prefix: $prefix" >&2
    exit 1
  }
  mkdir -p "$prefix"
  require_binary
  local stage
  stage="$(mktemp -d "$prefix/.corral-ui.stage.XXXXXX")"
  cleanup_other() {
    if [[ -n "$stage" && -e "$stage" ]]; then rm -rf "$stage"; fi
  }
  trap cleanup_other EXIT
  mkdir -p "$stage/bin"
  cp "$UI_BIN" "$stage/bin/corrald-ui"
  chmod +x "$stage/bin/corrald-ui"
  commit_files "$stage" "$prefix" bin/corrald-ui
  echo "   ✓ installed. Launch: $prefix/bin/corrald-ui"
)

case "$INSTALL_PLATFORM" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *) install_other ;;
esac
