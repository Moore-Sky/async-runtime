#!/usr/bin/env bash
set -euo pipefail

binary="$1"
shift

if [[ -n "${ADB:-}" ]]; then
    adb_command="$ADB"
elif command -v adb >/dev/null 2>&1; then
    adb_command="adb"
else
    adb_command="adb.exe"
fi
remote_binary="/data/local/tmp/async-runtime-test"
"$adb_command" push "$binary" "$remote_binary" >/dev/null
"$adb_command" shell chmod 755 "$remote_binary"

set +e
"$adb_command" shell "$remote_binary" "$@"
status=$?
set -e

"$adb_command" shell rm -f "$remote_binary" >/dev/null
exit "$status"
