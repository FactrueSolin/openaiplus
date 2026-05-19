#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

BIN_NAME="${OPENAIPLUS_BIN_NAME:-openaiplus}"
LABEL="${OPENAIPLUS_LAUNCHD_LABEL:-ai.actrue.openaiplus}"
INSTALL_DIR="${OPENAIPLUS_INSTALL_DIR:-/usr/local/openaiplus}"
PLIST_PATH="${OPENAIPLUS_PLIST_PATH:-/Library/LaunchDaemons/$LABEL.plist}"
LOG_DIR="${OPENAIPLUS_LOG_DIR:-/var/log/openaiplus}"
RUST_LOG_VALUE="${RUST_LOG:-info}"
CARGO_BIN="${CARGO:-cargo}"

SERVICE_TARGET="system/$LABEL"
BIN_PATH="$INSTALL_DIR/$BIN_NAME"
CONFIG_PATH="$INSTALL_DIR/config.toml"
OUT_LOG="$LOG_DIR/stdout.log"
ERR_LOG="$LOG_DIR/stderr.log"

usage() {
    cat <<USAGE
Usage: bash just/macos-service.sh <command>

Commands:
  build       Build target/release/$BIN_NAME
  deploy      Build, overwrite installed files, install plist, and start service
  start       Load and start the LaunchDaemon
  stop        Stop and unload the LaunchDaemon
  restart     Restart the LaunchDaemon
  status      Print launchd status
  logs        Show recent logs (LINES=100 by default)
  follow      Follow logs (LINES=100 by default)
  health      Request http://127.0.0.1:3000/healthz by default
  uninstall   Stop service and remove the plist; installed files are kept

Environment overrides:
  OPENAIPLUS_INSTALL_DIR     Default: /usr/local/openaiplus
  OPENAIPLUS_LAUNCHD_LABEL   Default: ai.actrue.openaiplus
  OPENAIPLUS_PLIST_PATH      Default: /Library/LaunchDaemons/<label>.plist
  OPENAIPLUS_LOG_DIR         Default: /var/log/openaiplus
  OPENAIPLUS_CONFIG_SOURCE   Default: ./config.toml when present
  OPENAIPLUS_HEALTH_HOST     Default: 127.0.0.1
  OPENAIPLUS_HEALTH_PORT     Default: 3000
USAGE
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

require_macos() {
    [ "$(uname -s)" = "Darwin" ] || fail "macOS service commands must run on macOS"
}

require_absolute_path() {
    case "$2" in
        /*) ;;
        *) fail "$1 must be an absolute path: $2" ;;
    esac
}

require_safe_paths() {
    require_absolute_path OPENAIPLUS_INSTALL_DIR "$INSTALL_DIR"
    require_absolute_path OPENAIPLUS_PLIST_PATH "$PLIST_PATH"
    require_absolute_path OPENAIPLUS_LOG_DIR "$LOG_DIR"

    [ "$INSTALL_DIR" != "/" ] || fail "OPENAIPLUS_INSTALL_DIR cannot be /"
    [ "$LOG_DIR" != "/" ] || fail "OPENAIPLUS_LOG_DIR cannot be /"
}

require_sudo() {
    require_command sudo
    sudo -v
}

xml_escape() {
    printf '%s' "$1" \
        | sed \
            -e 's/&/\&amp;/g' \
            -e 's/</\&lt;/g' \
            -e 's/>/\&gt;/g' \
            -e 's/"/\&quot;/g' \
            -e "s/'/\&apos;/g"
}

build_binary() {
    require_command "$CARGO_BIN"
    (cd "$ROOT_DIR" && "$CARGO_BIN" build --release)
}

install_artifacts() {
    require_safe_paths

    local built_bin="$ROOT_DIR/target/release/$BIN_NAME"
    [ -f "$built_bin" ] || fail "release binary not found: $built_bin"
    [ -d "$ROOT_DIR/static" ] || fail "static directory not found: $ROOT_DIR/static"

    sudo install -d -m 755 "$INSTALL_DIR" "$LOG_DIR"
    sudo install -m 755 "$built_bin" "$BIN_PATH"

    sudo rm -rf "$INSTALL_DIR/static"
    sudo install -d -m 755 "$INSTALL_DIR/static"
    sudo cp -R "$ROOT_DIR/static/." "$INSTALL_DIR/static/"
    sudo chmod -R a+rX "$INSTALL_DIR/static"

    local config_source="${OPENAIPLUS_CONFIG_SOURCE:-$ROOT_DIR/config.toml}"
    if [ -f "$config_source" ]; then
        sudo install -m 644 "$config_source" "$CONFIG_PATH"
    elif [ ! -f "$CONFIG_PATH" ] && [ -f "$ROOT_DIR/config.example.toml" ]; then
        sudo install -m 644 "$ROOT_DIR/config.example.toml" "$CONFIG_PATH"
    fi

    sudo touch "$OUT_LOG" "$ERR_LOG"
    sudo chmod 644 "$OUT_LOG" "$ERR_LOG"
}

write_plist() {
    require_safe_paths

    local plist_tmp
    plist_tmp="$(mktemp)"

    local label_xml bin_xml workdir_xml config_xml out_xml err_xml rust_log_xml
    label_xml="$(xml_escape "$LABEL")"
    bin_xml="$(xml_escape "$BIN_PATH")"
    workdir_xml="$(xml_escape "$INSTALL_DIR")"
    config_xml="$(xml_escape "$CONFIG_PATH")"
    out_xml="$(xml_escape "$OUT_LOG")"
    err_xml="$(xml_escape "$ERR_LOG")"
    rust_log_xml="$(xml_escape "$RUST_LOG_VALUE")"

    cat >"$plist_tmp" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label_xml</string>
    <key>ProgramArguments</key>
    <array>
        <string>$bin_xml</string>
    </array>
    <key>WorkingDirectory</key>
    <string>$workdir_xml</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>OPENAIPLUS_CONFIG</key>
        <string>$config_xml</string>
        <key>RUST_LOG</key>
        <string>$rust_log_xml</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$out_xml</string>
    <key>StandardErrorPath</key>
    <string>$err_xml</string>
</dict>
</plist>
PLIST

    sudo install -m 644 "$plist_tmp" "$PLIST_PATH"
    rm -f "$plist_tmp"
    sudo chown root:wheel "$PLIST_PATH"
}

is_loaded() {
    sudo launchctl print "$SERVICE_TARGET" >/dev/null 2>&1
}

load_service() {
    require_safe_paths
    [ -f "$PLIST_PATH" ] || fail "plist not found: $PLIST_PATH"
    sudo launchctl bootstrap system "$PLIST_PATH"
    sudo launchctl enable "$SERVICE_TARGET"
}

stop_service() {
    if is_loaded; then
        sudo launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1 \
            || sudo launchctl bootout system "$PLIST_PATH" >/dev/null 2>&1 \
            || fail "failed to stop $SERVICE_TARGET"
        printf 'stopped %s\n' "$SERVICE_TARGET"
    else
        printf '%s is not loaded\n' "$SERVICE_TARGET"
    fi
}

start_service() {
    require_macos
    require_sudo

    if is_loaded; then
        sudo launchctl kickstart -k "$SERVICE_TARGET"
    else
        load_service
    fi
    printf 'started %s\n' "$SERVICE_TARGET"
}

restart_service() {
    require_macos
    require_sudo

    if is_loaded; then
        sudo launchctl kickstart -k "$SERVICE_TARGET"
    else
        load_service
    fi
    printf 'restarted %s\n' "$SERVICE_TARGET"
}

deploy_service() {
    require_macos
    build_binary
    require_sudo

    if is_loaded; then
        stop_service
    fi

    install_artifacts
    write_plist
    load_service
    printf 'deployed %s to %s\n' "$SERVICE_TARGET" "$INSTALL_DIR"
}

status_service() {
    require_macos
    require_sudo
    if is_loaded; then
        sudo launchctl print "$SERVICE_TARGET"
    else
        printf '%s is not loaded\n' "$SERVICE_TARGET"
    fi
}

show_logs() {
    require_macos
    require_sudo

    local lines="${LINES:-100}"
    local mode="${1:-tail}"
    local files=()

    [ -e "$OUT_LOG" ] && files+=("$OUT_LOG")
    [ -e "$ERR_LOG" ] && files+=("$ERR_LOG")

    if [ "${#files[@]}" -eq 0 ]; then
        printf 'no log files found: %s %s\n' "$OUT_LOG" "$ERR_LOG"
        return 0
    fi

    if [ "$mode" = "follow" ]; then
        sudo tail -n "$lines" -f "${files[@]}"
    else
        sudo tail -n "$lines" "${files[@]}"
    fi
}

health_check() {
    local host="${OPENAIPLUS_HEALTH_HOST:-127.0.0.1}"
    local port="${OPENAIPLUS_HEALTH_PORT:-3000}"
    require_command curl
    curl -fsS "http://$host:$port/healthz"
    printf '\n'
}

uninstall_service() {
    require_macos
    require_sudo


    stop_service
    if [ -f "$PLIST_PATH" ]; then
        sudo rm -f "$PLIST_PATH"
        printf 'removed %s\n' "$PLIST_PATH"
    fi
    printf 'kept installed files in %s and logs in %s\n' "$INSTALL_DIR" "$LOG_DIR"
}

command_name="${1:-}"
case "$command_name" in
    build)
        build_binary
        ;;
    deploy)
        deploy_service
        ;;
    start)
        start_service
        ;;
    stop)
        require_macos
        require_sudo
        stop_service
        ;;
    restart)
        restart_service
        ;;
    status)
        status_service
        ;;
    logs)
        show_logs tail
        ;;
    follow)
        show_logs follow
        ;;
    health)
        health_check
        ;;
    uninstall)
        uninstall_service
        ;;
    -h|--help|help|'')
        usage
        ;;
    *)
        usage >&2
        fail "unknown command: $command_name"
        ;;
esac
