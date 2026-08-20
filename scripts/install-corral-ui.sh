#!/usr/bin/env bash
# install-corral-ui.sh — stage and transactionally install the Corral desktop UI
#
# The staging directory is created beside the destination. macOS commits keep
# the app transaction in one parent directory; Linux and Other first require
# every payload parent to report the same device, then use same-filesystem
# renames. Existing payloads are moved to rollback storage before the staged
# payload is renamed into place; any failed move restores the prior payload.
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

reject_parent_traversal() {
  local path="$1"
  local label="$2"
  case "/$path/" in
    */../*)
      echo "!! refusing unsafe $label containing ..: $path" >&2
      return 1
      ;;
  esac
}

reject_destination_linebreaks() {
  local path="$1"
  local label="$2"
  case "$path" in
    *$'\n'*|*$'\r'*)
      echo "!! refusing $label containing a newline or carriage return" >&2
      return 1
      ;;
  esac
}

stat_device() {
  local path="$1"
  case "$(uname -s)" in
    Darwin|FreeBSD|OpenBSD|NetBSD) stat -f '%d' "$path" ;;
    *) stat -c '%d' "$path" ;;
  esac
}

device_id_for_path() {
  local path="$1"
  local label="$2"
  local current="$path"
  while [[ ! -e "$current" && ! -L "$current" ]]; do
    [[ "$current" != "/" ]] || {
      echo "!! could not find an existing ancestor for $label: $path" >&2
      return 1
    }
    current="$(dirname "$current")"
  done
  [[ -d "$current" ]] || {
    echo "!! $label is not a directory: $path" >&2
    return 1
  }
  local device
  device="$(stat_device "$current")" || {
    echo "!! could not read the filesystem device for $label: $path" >&2
    return 1
  }
  [[ -n "$device" ]] || {
    echo "!! filesystem device is empty for $label: $path" >&2
    return 1
  }
  printf '%s' "$device"
}

assert_same_filesystem() {
  local reference="$1"
  local label="$2"
  shift 2
  local reference_device
  reference_device="$(device_id_for_path "$reference" "$label reference")" || return 1
  local candidate
  local candidate_device
  for candidate in "$@"; do
    candidate_device="$(device_id_for_path "$candidate" "$label target parent")" || return 1
    [[ "$candidate_device" == "$reference_device" ]] || {
      echo "!! refusing $label across filesystems: $reference and $candidate" >&2
      return 1
    }
  done
}

canonicalize_directory() {
  local path="$1"
  local label="$2"
  [[ -n "$path" ]] || {
    echo "!! refusing empty $label" >&2
    return 1
  }
  reject_destination_linebreaks "$path" "$label" || return 1
  reject_parent_traversal "$path" "$label" || return 1
  case "$path" in
    /*) ;;
    *) path="$(pwd -P)/$path" ;;
  esac

  local current="$path"
  local parent
  local component
  local canonical
  local -a missing=()
  while [[ ! -e "$current" && ! -L "$current" ]]; do
    [[ "$current" != "/" ]] || {
      echo "!! could not resolve $label: $path" >&2
      return 1
    }
    parent="$(dirname "$current")"
    component="$(basename "$current")"
    [[ -n "$component" && "$component" != "." && "$component" != ".." ]] || {
      echo "!! could not resolve $label: $path" >&2
      return 1
    }
    if [[ "${#missing[@]}" -eq 0 ]]; then
      missing=("$component")
    else
      missing=("$component" "${missing[@]}")
    fi
    current="$parent"
  done
  [[ -d "$current" ]] || {
    echo "!! $label is not a directory: $path" >&2
    return 1
  }
  canonical="$(cd -P -- "$current" && pwd -P)" || {
    echo "!! could not canonicalize $label: $path" >&2
    return 1
  }
  if [[ "${#missing[@]}" -gt 0 ]]; then
    for component in "${missing[@]}"; do
      if [[ "$canonical" == "/" ]]; then
        canonical="/$component"
      else
        canonical="$canonical/$component"
      fi
    done
  fi
  printf '%s' "$canonical"
}

canonicalize_destination() {
  local path="$1"
  local label="$2"
  [[ -n "$path" ]] || {
    echo "!! refusing empty $label" >&2
    return 1
  }
  reject_destination_linebreaks "$path" "$label" || return 1
  reject_parent_traversal "$path" "$label" || return 1
  local leaf="$(basename "$path")"
  [[ -n "$leaf" && "$leaf" != "." && "$leaf" != ".." ]] || {
    echo "!! refusing unsafe $label: $path" >&2
    return 1
  }
  local parent
  parent="$(canonicalize_directory "$(dirname "$path")" "$label parent")" || return 1
  [[ "$parent" != "/" ]] || {
    echo "!! refusing broad $label parent: $path" >&2
    return 1
  }
  printf '%s/%s' "$parent" "$leaf"
}

assert_no_symlink_components() {
  local path="$1"
  local label="$2"
  [[ "$path" == /* ]] || {
    echo "!! refusing non-absolute $label: $path" >&2
    return 1
  }
  local current="/"
  local remainder="${path#/}"
  local component
  while [[ -n "$remainder" ]]; do
    component="${remainder%%/*}"
    if [[ "$remainder" == "$component" ]]; then
      remainder=""
    else
      remainder="${remainder#*/}"
    fi
    [[ -z "$component" || "$component" == "." ]] && continue
    [[ "$component" != ".." ]] || {
      echo "!! refusing unsafe $label containing ..: $path" >&2
      return 1
    }
    if [[ "$current" == "/" ]]; then
      current="/$component"
    else
      current="$current/$component"
    fi
    if [[ -L "$current" ]]; then
      echo "!! refusing symlink in $label: $current" >&2
      return 1
    fi
  done
}

assert_safe_target() {
  local path="$1"
  local label="$2"
  [[ "$path" != "/" && -n "$path" ]] || {
    echo "!! refusing root $label: $path" >&2
    return 1
  }
  assert_no_symlink_components "$path" "$label"
}

rollback_directory() {
  local destination="$1"
  local rollback_dir="$2"
  local had_destination="$3"
  local payload_rollback_ok=0
  local cleanup_ok=0

  if ! assert_safe_target "$destination" "desktop app rollback target"; then
    payload_rollback_ok=1
  elif [[ -e "$destination" ]]; then
    if ! rm -rf "$destination"; then
      payload_rollback_ok=1
    fi
  fi
  if [[ "$payload_rollback_ok" == "0" && "$had_destination" == "1" ]]; then
    if ! mv "$rollback_dir/previous" "$destination"; then
      payload_rollback_ok=1
    fi
  fi
  if [[ "$payload_rollback_ok" == "0" ]]; then
    if ! rm -rf "$rollback_dir"; then
      cleanup_ok=1
    fi
  fi
  if [[ "$payload_rollback_ok" != "0" ]]; then
    echo "!! payload rollback failed; prior desktop payload is retained in rollback directory: $rollback_dir" >&2
    return 1
  fi
  if [[ "$cleanup_ok" != "0" ]]; then
    if [[ "$had_destination" == "1" ]]; then
      echo "!! prior desktop payload restored; rollback directory cleanup failed; inspect rollback directory: $rollback_dir" >&2
    else
      echo "!! staged desktop payload removed; rollback directory cleanup failed; inspect rollback directory: $rollback_dir" >&2
    fi
    return 1
  fi
  return 0
}

commit_directory() {
  local stage="$1"
  local destination="$2"
  local parent
  parent="$(dirname "$destination")"
  assert_safe_target "$parent" "desktop app transaction parent"
  assert_safe_target "$destination" "desktop app destination"
  [[ "$(dirname "$stage")" == "$parent" ]] || {
    echo "!! refusing cross-directory desktop app staging: $stage" >&2
    return 1
  }
  local rollback_dir
  rollback_dir="$(mktemp -d "$parent/.corral-ui.rollback.XXXXXX")"
  local had_destination=0

  if [[ -e "$destination" || -L "$destination" ]]; then
    if ! mv "$destination" "$rollback_dir/previous"; then
      rm -rf "$rollback_dir"
      echo "!! could not move the existing desktop payload into rollback storage" >&2
      return 1
    fi
    had_destination=1
  fi

  if ! mv "$stage" "$destination"; then
    if ! rollback_directory "$destination" "$rollback_dir" "$had_destination"; then
      return 1
    fi
    echo "!! could not install the staged desktop payload; previous desktop payload restored" >&2
    return 1
  fi

  if ! rm -rf "$rollback_dir"; then
    echo "!! installed desktop payload; rollback copy retained at $rollback_dir" >&2
  fi
}

commit_files() {
  local stage="$1"
  local prefix="$2"
  shift 2
  local -a relative_paths=("$@")
  assert_safe_target "$prefix" "desktop payload prefix"
  [[ "$(dirname "$stage")" == "$prefix" ]] || {
    echo "!! refusing cross-directory desktop payload staging: $stage" >&2
    return 1
  }
  local relative
  local target
  local target_parent
  local -a target_parents=()
  for relative in "${relative_paths[@]}"; do
    assert_safe_target "$prefix/$relative" "desktop payload target"
    target_parent="$(dirname "$prefix/$relative")"
    [[ -d "$target_parent" ]] || {
      echo "!! desktop payload parent is missing: $target_parent" >&2
      return 1
    }
    target_parents+=("$target_parent")
  done
  assert_same_filesystem "$prefix" "desktop payload" "${target_parents[@]}"
  local rollback_dir
  rollback_dir="$(mktemp -d "$prefix/.corral-ui.rollback.XXXXXX")"
  local -a backed_up=()
  local -a installed=()

  rollback_files() {
    local payload_rollback_ok=0
    local cleanup_ok=0
    local index
    local relative
    local target
    local backup

    for ((index = ${#installed[@]} - 1; index >= 0; index--)); do
      relative="${installed[$index]}"
      target="$prefix/$relative"
      if ! assert_safe_target "$target" "desktop payload rollback target"; then
        payload_rollback_ok=1
        continue
      fi
      if [[ -e "$target" ]]; then
        if ! rm -rf "$target"; then
          payload_rollback_ok=1
          continue
        fi
      fi
    done
    for ((index = ${#backed_up[@]} - 1; index >= 0; index--)); do
      relative="${backed_up[$index]}"
      target="$prefix/$relative"
      backup="$rollback_dir/$relative"
      if ! assert_safe_target "$target" "desktop payload rollback target"; then
        payload_rollback_ok=1
        continue
      fi
      if [[ ! -d "$(dirname "$target")" ]]; then
        payload_rollback_ok=1
        continue
      fi
      if [[ ! -e "$backup" && ! -L "$backup" ]]; then
        payload_rollback_ok=1
        continue
      fi
      if ! mv "$backup" "$target"; then
        payload_rollback_ok=1
      fi
    done
    if [[ "$payload_rollback_ok" == "0" ]]; then
      if ! rm -rf "$rollback_dir"; then
        cleanup_ok=1
      fi
    fi
    if [[ "$payload_rollback_ok" != "0" ]]; then
      echo "!! payload rollback failed; prior desktop payload is retained in rollback directory: $rollback_dir" >&2
      return 1
    fi
    if [[ "$cleanup_ok" != "0" ]]; then
      if [[ "${#backed_up[@]}" -gt 0 ]]; then
        echo "!! prior desktop payload restored; rollback directory cleanup failed; inspect rollback directory: $rollback_dir" >&2
      else
        echo "!! staged desktop payload removed; rollback directory cleanup failed; inspect rollback directory: $rollback_dir" >&2
      fi
      return 1
    fi
    return 0
  }

  local backup
  for relative in "${relative_paths[@]}"; do
    target="$prefix/$relative"
    assert_safe_target "$target" "desktop payload target"
    if [[ -e "$target" ]]; then
      backup="$rollback_dir/$relative"
      mkdir -p "$(dirname "$backup")"
      if ! mv "$target" "$backup"; then
        rollback_files || true
        echo "!! could not move an existing desktop payload into rollback storage" >&2
        return 1
      fi
      backed_up+=("$relative")
    fi
  done

  for relative in "${relative_paths[@]}"; do
    target="$prefix/$relative"
    assert_safe_target "$target" "desktop payload target"
    [[ -d "$(dirname "$target")" ]] || {
      echo "!! desktop payload parent disappeared: $(dirname "$target")" >&2
      if ! rollback_files; then
        return 1
      fi
      return 1
    }
    if ! mv "$stage/$relative" "$target"; then
      if ! rollback_files; then
        return 1
      fi
      echo "!! could not install the staged desktop payload; previous desktop payload restored" >&2
      return 1
    fi
    installed+=("$relative")
  done

  if ! rm -rf "$rollback_dir"; then
    echo "!! installed desktop payload; rollback copies retained at $rollback_dir" >&2
  fi
}

install_macos() (
  set -euo pipefail
  reject_destination_linebreaks "$MACOS_APP_DEST" "macOS app destination"
  MACOS_APP_DEST="$(canonicalize_destination "$MACOS_APP_DEST" "macOS app destination")" || exit 1
  local app_parent
  app_parent="$(dirname "$MACOS_APP_DEST")"
  assert_safe_target "$app_parent" "macOS app parent"
  assert_safe_target "$MACOS_APP_DEST" "macOS app destination"
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
  mkdir -p "$app_parent"
  assert_safe_target "$app_parent" "macOS app parent"
  assert_safe_target "$MACOS_APP_DEST" "macOS app destination"

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

desktop_exec_quote() {
  local value="$1"
  local output='"'
  local character
  while [[ -n "$value" ]]; do
    character="${value%"${value#?}"}"
    value="${value#?}"
    case "$character" in
      \\)
        output+='\\'
        output+='\\'
        ;;
      \")
        output+="\\"
        output+="\\"
        output+="\\"
        output+='"'
        ;;
      '$')
        output+="\\"
        output+="\\"
        output+='$'
        ;;
      =)
        echo "!! Linux executable path contains Desktop Entry '='" >&2
        return 1
        ;;
      '`')
        output+="\\"
        output+="\\"
        output+='`'
        ;;
      %) output+='%%' ;;
      $'\n'|$'\r')
        echo "!! Linux executable path contains a desktop-entry newline" >&2
        return 1
        ;;
      *) output+="$character" ;;
    esac
  done
  output+='"'
  printf '%s' "$output"
}

install_linux() (
  set -euo pipefail
  reject_destination_linebreaks "$LINUX_PREFIX" "Linux install prefix"
  LINUX_PREFIX="$(canonicalize_directory "$LINUX_PREFIX" "Linux install prefix")" || exit 1
  assert_safe_target "$LINUX_PREFIX" "Linux install prefix"
  require_binary
  [[ -f "$LINUX_ICON" ]] || {
    echo "!! Linux desktop icon missing: $LINUX_ICON" >&2
    exit 1
  }
  local desktop_exec
  desktop_exec="$(desktop_exec_quote "$LINUX_PREFIX/bin/corrald-ui")"
  local -a payload_parents=(
    "$LINUX_PREFIX/bin"
    "$LINUX_PREFIX/share/applications"
    "$LINUX_PREFIX/share/icons/hicolor/256x256/apps"
  )
  local payload_parent
  require_command stat
  for payload_parent in "${payload_parents[@]}"; do
    assert_safe_target "$payload_parent" "Linux payload parent"
  done
  assert_same_filesystem "$LINUX_PREFIX" "Linux payload parents" "${payload_parents[@]}"
  mkdir -p "$LINUX_PREFIX"
  assert_safe_target "$LINUX_PREFIX" "Linux install prefix"
  for payload_parent in "${payload_parents[@]}"; do
    assert_safe_target "$payload_parent" "Linux payload parent"
  done
  mkdir -p \
    "$LINUX_PREFIX/bin" \
    "$LINUX_PREFIX/share/applications" \
    "$LINUX_PREFIX/share/icons/hicolor/256x256/apps"
  assert_same_filesystem "$LINUX_PREFIX" "Linux payload parents" "${payload_parents[@]}"
  assert_safe_target "$LINUX_PREFIX/bin/corrald-ui" "Linux executable target"
  assert_safe_target "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png" "Linux icon target"
  assert_safe_target "$LINUX_PREFIX/share/applications/corral.desktop" "Linux desktop-entry target"

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
  reject_destination_linebreaks "$OTHER_PREFIX" "Other install prefix"
  local prefix
  prefix="$(canonicalize_directory "$OTHER_PREFIX" "Other install prefix")" || exit 1
  assert_safe_target "$prefix" "Other install prefix"
  require_binary
  local payload_parent="$prefix/bin"
  require_command stat
  assert_safe_target "$payload_parent" "Other payload parent"
  assert_same_filesystem "$prefix" "Other payload parent" "$payload_parent"
  mkdir -p "$prefix"
  assert_safe_target "$prefix" "Other install prefix"
  assert_safe_target "$payload_parent" "Other payload parent"
  mkdir -p "$prefix/bin"
  assert_same_filesystem "$prefix" "Other payload parent" "$payload_parent"
  assert_safe_target "$prefix/bin/corrald-ui" "Other executable target"
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
