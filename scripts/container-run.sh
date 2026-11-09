#!/bin/sh
# Tooling arguments only; all application settings remain in the selected YAML.
set -eu
engine=${1:-podman}
image=${2:-localhost/pullboard:latest}
config=${3:-}
port=${4:-8080}
name=${5:-}
uid=$(id -u)
gid=$(id -g)
set -- run --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
    --user "$uid:$gid" \
    -p "127.0.0.1:$port:8080"
case $("$engine" --version) in
    *[Pp]odman*) set -- "$@" --userns=keep-id \
        --tmpfs /data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U ;;
    *) set -- "$@" --tmpfs "/data:rw,nosuid,nodev,noexec,size=64m,uid=$uid,gid=$gid,mode=0700" ;;
esac
if [ -n "$name" ]; then
    set -- "$@" --name "$name"
fi
if [ -n "$config" ]; then
    if [ ! -f "$config" ]; then
        echo 'Configuration file does not exist.' >&2
        exit 1
    fi
    config=$(realpath "$config")
    set -- "$@" -v "$config:/app/config.yaml:ro,Z"
fi
exec "$engine" "$@" "$image"
