#!/bin/sh
# GPL-3.0-or-later. Disposable launcher; download locations belong to the publisher.
set -eu

fail() {
    printf '%s\n' "pullboard-setup: $*" >&2
    exit 1
}

nightly=false
need_url=false
for argument do
    if [ "$need_url" = true ]; then
        case "$argument" in ''|--*) fail '--jira-cloud-url requires an HTTPS tenant root.' ;; esac
        need_url=false
        continue
    fi
    case "$argument" in
        --nightly) nightly=true ;;
        --no-color) ;;
        --jira-cloud-url) need_url=true ;;
        --jira-cloud-url=) fail '--jira-cloud-url requires an HTTPS tenant root.' ;;
        --jira-cloud-url=*) ;;
        -h|--help)
            cat <<'HELP'
Usage: pullboard-setup.sh [--nightly] [--no-color] [--jira-cloud-url HTTPS_TENANT_ROOT]

Run the full interactive PullBoard setup wizard on Linux x86-64.
Stable is the default; --nightly downloads the rolling prerelease and preselects
the nightly image (editable in the wizard). The Jira URL is only an editable prefill.
Enter credentials in the wizard, never as command-line or environment values.
Requires a terminal, curl, gzip, sha256sum and standard POSIX utilities.
Downloads are private, checksum-verified, and removed on exit; nothing is installed
in ~/.local/bin. Checksums detect corruption, not independent publisher authenticity.
HELP
            exit 0 ;;
        *) fail 'Unsupported option. Use --help; credentials must be entered in the wizard.' ;;
    esac
done
[ "$need_url" = false ] || fail '--jira-cloud-url requires an HTTPS tenant root.'

[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] ||
    fail 'Only Linux x86-64 is supported; use the manual container guide on other systems.'
for utility in curl gzip sha256sum mktemp awk chmod rm rmdir; do
    command -v "$utility" >/dev/null 2>&1 || fail "Missing $utility; install it with your system package manager."
done
[ -t 1 ] || fail 'Run from an interactive terminal (stdout must be a terminal).'
( : </dev/tty ) 2>/dev/null || fail 'No controlling terminal. Run this command in an interactive terminal.'

base=https://github.com/pbrezina/pullboard/releases/latest/download
if [ "$nightly" = true ]; then
    base=https://github.com/pbrezina/pullboard/releases/download/nightly
fi
asset=pullboard-setup-linux-x86_64.gz
umask 077
temporary=$(mktemp -d "${TMPDIR:-/tmp}/pullboard-setup.XXXXXXXXXX") || fail 'Cannot create a private temporary directory.'
cleanup() {
    rm -f "$temporary/$asset" "$temporary/SHA256SUMS" \
        "$temporary/expected.sha256" "$temporary/pullboard-setup"
    rmdir "$temporary" 2>/dev/null || :
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

download() {
    # Ignore curlrc; never permit an HTTP redirect downgrade or arbitrary mirror.
    curl --disable --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 180 --output "$temporary/$1" "$base/$1" ||
        fail 'Download failed. Check connectivity and release availability, then retry.'
}
printf '%s\n' 'Downloading PullBoard setup from GitHub Releases…' >&2
download SHA256SUMS
download "$asset"
# Check only the fixed asset name, never paths supplied by the checksum manifest.
awk -v name="$asset" '$2 == name { count++; checksum=$1 }
    END {
        if (count != 1 || length(checksum) != 64 || checksum ~ /[^0-9a-fA-F]/) exit 1;
        print checksum "  " name;
    }' "$temporary/SHA256SUMS" > "$temporary/expected.sha256" ||
    fail 'Invalid release checksum manifest. Nothing was executed; retry later.'
(cd "$temporary" && sha256sum -c expected.sha256 >/dev/null 2>&1) ||
    fail 'Checksum mismatch. Nothing was executed; retry (a rolling release may be updating).'
gzip -dc "$temporary/$asset" > "$temporary/pullboard-setup" ||
    fail 'Cannot decompress the verified download. Nothing was executed.'
chmod 700 "$temporary/pullboard-setup"
# Piped launchers consume stdin; give the prompts their controlling terminal back.
# An asynchronous child plus wait lets PID-directed signals interrupt the shell.
child_pid=
pending_signal=
cancel_status=
interrupted=false
cancel_child() {
    interrupted=true
    cancel_status=${cancel_status:-$2}
    if [ -n "$child_pid" ]; then
        kill -s "$1" "$child_pid" 2>/dev/null || :
    else
        # A signal can arrive between spawning the child and assigning $!.
        pending_signal=$1
    fi
}
# The wizard handles TERM gracefully; its default HUP action would skip terminal
# restoration. Preserve the launcher's HUP exit status while requesting safe cleanup.
trap 'cancel_child TERM 129' HUP
trap 'cancel_child INT 130' INT
trap 'cancel_child TERM 143' TERM
"$temporary/pullboard-setup" "$@" </dev/tty &
child_pid=$!
if [ -n "$pending_signal" ]; then
    kill -s "$pending_signal" "$child_pid" 2>/dev/null || :
fi
while :; do
    interrupted=false
    if wait "$child_pid"; then
        status=0
    else
        status=$?
    fi
    # A trapped signal interrupts wait before the child finishes. Keep waiting
    # for graceful cancellation/reporting; never clean up a running installer.
    [ "$interrupted" = true ] || break
done
child_pid=
exit "${cancel_status:-$status}"
