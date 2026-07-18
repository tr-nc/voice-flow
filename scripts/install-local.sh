#!/bin/sh

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

require_command npm
require_command cargo

printf 'Installing JavaScript dependencies...\n'
npm ci

case "$(uname -s)" in
  Darwin)
    require_command ditto

    printf 'Building the macOS application...\n'
    npm run tauri -- build --bundles app

    source_app="$repo_root/src-tauri/target/release/bundle/macos/Voice Flow.app"
    installed_app="$HOME/Applications/Voice Flow.app"
    [ -d "$source_app" ] || fail "Tauri did not create $source_app"

    mkdir -p "$HOME/Applications"
    rm -rf "$installed_app"
    ditto "$source_app" "$installed_app"

    printf '\nInstalled Voice Flow at:\n  %s\n' "$installed_app"
    printf 'Launch it with:\n  open "%s"\n' "$installed_app"
    printf 'On first launch, grant Microphone and Accessibility access when macOS asks.\n'
    ;;

  Linux)
    printf 'Building the Linux application...\n'
    npm run tauri -- build --no-bundle

    source_binary="$repo_root/src-tauri/target/release/voice-flow"
    installed_binary="$HOME/.local/bin/voice-flow"
    desktop_file="$HOME/.local/share/applications/voice-flow.desktop"
    icon_file="$HOME/.local/share/icons/hicolor/128x128/apps/dev.voiceflow.desktop.png"
    [ -x "$source_binary" ] || fail "Tauri did not create $source_binary"

    install -d "$HOME/.local/bin"
    install -m 0755 "$source_binary" "$installed_binary"
    install -d "$(dirname -- "$icon_file")"
    install -m 0644 "$repo_root/src-tauri/icons/128x128.png" "$icon_file"
    install -d "$(dirname -- "$desktop_file")"

    desktop_tmp=$(mktemp "${TMPDIR:-/tmp}/voice-flow.desktop.XXXXXX")
    trap 'rm -f "$desktop_tmp"' EXIT HUP INT TERM
    {
      printf '%s\n' '[Desktop Entry]'
      printf '%s\n' 'Type=Application'
      printf '%s\n' 'Name=Voice Flow'
      printf '%s\n' 'Comment=Real-time voice input at the active cursor'
      printf 'Exec="%s"\n' "$installed_binary"
      printf 'TryExec=%s\n' "$installed_binary"
      printf '%s\n' 'Icon=dev.voiceflow.desktop'
      printf '%s\n' 'Terminal=false'
      printf '%s\n' 'Categories=Utility;'
      printf '%s\n' 'StartupNotify=true'
    } >"$desktop_tmp"
    install -m 0644 "$desktop_tmp" "$desktop_file"
    rm -f "$desktop_tmp"
    trap - EXIT HUP INT TERM

    if command -v update-desktop-database >/dev/null 2>&1; then
      update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
    fi

    printf '\nInstalled Voice Flow at:\n  %s\n' "$installed_binary"
    printf 'Launch Voice Flow from the application menu or run:\n  %s\n' "$installed_binary"

    if ! id -nG | tr ' ' '\n' | grep -qx input; then
      printf '\nwarning: this login session is not in the input group.\n' >&2
      printf 'Run `sudo usermod -aG input "$USER"`, then fully sign out and back in.\n' >&2
    fi
    if [ ! -w /dev/uinput ]; then
      printf '\nwarning: /dev/uinput is not writable; automatic insertion will be unavailable.\n' >&2
      printf 'Run `sudo modprobe uinput` and verify the active-session ACL.\n' >&2
    fi
    ;;

  *)
    fail "unsupported operating system: $(uname -s)"
    ;;
esac

if pgrep -x voice-flow >/dev/null 2>&1; then
  printf '\nVoice Flow is already running. Restart it to use this build.\n'
fi
