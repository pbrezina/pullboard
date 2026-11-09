#!/bin/sh
# Run ON THE HOST, explicitly, for a dedicated read/write Secret Service transport.
# No credentials are copied. Other session services are not exposed.
# Requires xdg-dbus-proxy. Keep the host keyring unlocked and use --userns=keep-id
# (or the same UID) when mounting ONLY this dedicated directory in the container.
set -eu
umask 077
if ! command -v xdg-dbus-proxy >/dev/null 2>&1; then
    echo 'xdg-dbus-proxy is required for the opt-in restricted bridge.' >&2
    exit 1
fi
: "${DBUS_SESSION_BUS_ADDRESS:?Host Secret Service session bus is unavailable}"
: "${XDG_RUNTIME_DIR:?Host runtime directory is unavailable}"
bridge_dir="$XDG_RUNTIME_DIR/pullboard-secret-service"
mkdir -p "$bridge_dir"
chmod 700 "$bridge_dir"
socket="$bridge_dir/bus"
if [ -e "$socket" ]; then
    echo 'A bridge socket already exists. Stop its owner before starting another bridge.' >&2
    exit 1
fi
echo "Restricted Secret Service bridge: $socket" >&2
echo 'Mount only this directory; select its socket using container DBUS_SESSION_BUS_ADDRESS.' >&2
exec xdg-dbus-proxy "$DBUS_SESSION_BUS_ADDRESS" "$socket" --filter --talk=org.freedesktop.secrets
