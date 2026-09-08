#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

# The optional /etc path allows fixture tests without changing the host system.
etc_dir="${1:-/etc}"
if ! ( . "$etc_dir/os-release"; [ "${ID:-}" = debian ] && [ "${VERSION_CODENAME:-}" = bullseye ] ); then
    echo "Linux CI sources require Debian Bullseye." >&2
    exit 1
fi

# Preserve the Bullseye ABI. Its final security index expired after LTS ended
# on 2026-08-31; disable expiry only for that source, retaining GPG verification.
# The security archive is not yet available on archive.debian.org.
mkdir -p "$etc_dir/apt/sources.list.d"
if [ -f "$etc_dir/apt/sources.list" ] && [ ! -e "$etc_dir/apt/sources.list.ci-backup" ]; then
    cp "$etc_dir/apt/sources.list" "$etc_dir/apt/sources.list.ci-backup"
fi
if [ -f "$etc_dir/apt/sources.list.d/debian.sources" ]; then
    mv "$etc_dir/apt/sources.list.d/debian.sources" "$etc_dir/apt/sources.list.d/debian.sources.ci-backup"
fi
cat > "$etc_dir/apt/sources.list" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://deb.debian.org/debian bullseye main
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://deb.debian.org/debian bullseye-updates main
deb [check-valid-until=no signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://deb.debian.org/debian-security bullseye-security main
SOURCES
