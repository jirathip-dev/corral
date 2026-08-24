#!/usr/bin/env bash
# lib-corral-update-path.sh — launchd PATH derivation for the corral auto-update.
#
# launchd runs com.corral.corrald-update with a minimal PATH
# (/usr/bin:/bin:/usr/sbin:/sbin), which omits Homebrew's bin. `gh` (git's
# HTTPS credential helper) lives in Homebrew's bin, so `git fetch origin main`
# fails with `gh: command not found` and the daemon silently drifts behind a
# manual rebuild. This derives a runtime PATH that prepends Homebrew's bin,
# ~/.local/bin, and the rustup cargo bin when present, so launchd jobs can find
# `gh` and cargo. It is a no-op when none are installed and must not fail under
# `set -u`.
#
# Source-only: defines corral_prepend_update_path(). It is not runnable directly.
set -euo pipefail

# corral_prepend_update_path [candidate-bin ...]
#   Prepends Homebrew's bin, ~/.local/bin, and ~/.cargo/bin to PATH when present.
#   Candidate bin dirs may be passed explicitly (used by tests); by default the
#   well-known Homebrew prefixes are tried. PATH is untouched when none exist.
corral_prepend_update_path() {
  local brew_bin=""
  local brew_path=""
  local candidate=""
  local cargo_bin=""
  local local_bin=""
  local extra=""
  local -a candidates=()

  if [[ "$#" -gt 0 ]]; then
    candidates=("$@")
  else
    candidates=(/opt/homebrew/bin /usr/local/bin /home/linuxbrew/.linuxbrew/bin)
  fi

  # Prefer an actual `brew` on PATH (covers a non-default prefix the user has
  # exported); otherwise fall back to the well-known prefixes, which handle the
  # launchd minimal-PATH case where brew is not on PATH at all.
  brew_path="$(command -v brew 2>/dev/null || true)"
  if [[ -n "$brew_path" ]]; then
    # Keep the dir exactly as it appears on PATH so the dedupe check below
    # matches; canonicalizing (pwd -P) can turn /var into /private/var and miss
    # an entry that is already present.
    brew_bin="$(dirname "$brew_path")"
  fi
  if [[ -z "$brew_bin" ]]; then
    for candidate in "${candidates[@]}"; do
      if [[ -d "$candidate" ]]; then
        brew_bin="$candidate"
        break
      fi
    done
  fi

  # rustup installs cargo/rustc under ~/.cargo/bin; launchd omits it too, so the
  # rebuild step in update-corral.sh would otherwise fail on a real update.
  if [[ -n "${HOME:-}" && -d "${HOME}/.cargo/bin" ]]; then
    cargo_bin="${HOME}/.cargo/bin"
  fi

  # User-local installs (including a user-installed `gh`) live under ~/.local/bin.
  # launchd does not inherit the interactive shell's user-local PATH entries.
  if [[ -n "${HOME:-}" && -d "${HOME}/.local/bin" ]]; then
    local_bin="${HOME}/.local/bin"
  fi

  for extra in "$brew_bin" "$cargo_bin" "$local_bin"; do
    if [[ -n "$extra" ]]; then
      case ":${PATH:-}:" in
        *":$extra:"*) ;;  # already present — leave as-is
        *) export PATH="$extra:${PATH:-}" ;;
      esac
    fi
  done
}
