#!/bin/sh

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

pid_is_running() {
  kill -0 "$1" 2>/dev/null
}

running_pids_from_list() {
  for pid in $1; do
    if pid_is_running "$pid"; then
      printf '%s\n' "$pid"
    fi
  done
}

stop_installed_instance() {
  pids=$1
  [ -n "$pids" ] || return 0

  printf 'Stopping the installed Voice Flow instance...\n'
  for pid in $pids; do
    kill -TERM "$pid" 2>/dev/null || true
  done

  attempts=0
  while [ "$attempts" -lt 5 ]; do
    remaining=$(running_pids_from_list "$pids")
    [ -n "$remaining" ] || return 0
    sleep 1
    attempts=$((attempts + 1))
  done

  printf 'Voice Flow did not exit in time; forcing the installed instance to stop...\n'
  for pid in $remaining; do
    kill -KILL "$pid" 2>/dev/null || true
  done
  sleep 1

  remaining=$(running_pids_from_list "$remaining")
  [ -z "$remaining" ] || fail "could not stop the installed Voice Flow instance (PID(s): $remaining)"
}

case "$(uname -s)" in
  Darwin)
    installed_app="$HOME/Applications/Voice Flow.app"
    installed_executable="$installed_app/Contents/MacOS/voice-flow"

    # Match argv[0] against the executable inside the installed app bundle. This
    # deliberately excludes target/ binaries launched by `cargo tauri dev`.
    running_pids=$(ps -ax -o pid= -o command= | awk -v executable="$installed_executable" '
      {
        pid = $1
        $1 = ""
        sub(/^[[:space:]]*/, "", $0)
        if ($0 == executable || index($0, executable " ") == 1) {
          print pid
        }
      }
    ')
    stop_installed_instance "$running_pids"

    printf 'Reinstalling Voice Flow...\n'
    npm run install:local

    [ -d "$installed_app" ] || fail "installed application not found: $installed_app"
    printf 'Starting the newly installed Voice Flow...\n'
    open "$installed_app"
    ;;

  Linux)
    installed_binary="$HOME/.local/bin/voice-flow"
    running_pids=''

    # /proc/PID/exe gives us an exact executable-path match. A process launched
    # from target/ therefore cannot be mistaken for the installed application.
    for proc_exe in /proc/[0-9]*/exe; do
      [ -L "$proc_exe" ] || continue
      executable=$(readlink "$proc_exe" 2>/dev/null || true)
      case "$executable" in
        "$installed_binary"|"$installed_binary (deleted)")
          pid=${proc_exe#/proc/}
          pid=${pid%/exe}
          running_pids="${running_pids}${running_pids:+ }${pid}"
          ;;
      esac
    done
    stop_installed_instance "$running_pids"

    printf 'Reinstalling Voice Flow...\n'
    npm run install:local

    [ -x "$installed_binary" ] || fail "installed executable not found: $installed_binary"
    printf 'Starting the newly installed Voice Flow...\n'
    if command -v setsid >/dev/null 2>&1; then
      setsid "$installed_binary" >/dev/null 2>&1 < /dev/null &
    else
      nohup "$installed_binary" >/dev/null 2>&1 < /dev/null &
    fi
    launched_pid=$!
    sleep 1
    pid_is_running "$launched_pid" || fail "the newly installed Voice Flow exited during startup"
    ;;

  *)
    fail "unsupported operating system: $(uname -s)"
    ;;
esac

printf 'Voice Flow was reinstalled and started successfully.\n'
