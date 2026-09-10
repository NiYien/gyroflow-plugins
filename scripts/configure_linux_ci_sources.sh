#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

# The optional /etc path allows fixture tests without changing the host system.
etc_dir="${1:-/etc}"
if ! ( . "$etc_dir/os-release"; [ "${ID:-}" = debian ] && [ "${VERSION_CODENAME:-}" = bullseye ] ); then
    echo "Linux CI sources require Debian Bullseye." >&2
    exit 1
fi

# Preserve the Bullseye ABI and pin indexes and packages to the same snapshot.
# Historical indexes need expiry disabled; GPG verification remains enabled.
mkdir -p "$etc_dir/apt/sources.list.d"
if [ -f "$etc_dir/apt/sources.list" ] && [ ! -e "$etc_dir/apt/sources.list.ci-backup" ]; then
    cp "$etc_dir/apt/sources.list" "$etc_dir/apt/sources.list.ci-backup"
fi
if [ -f "$etc_dir/apt/sources.list.d/debian.sources" ]; then
    mv "$etc_dir/apt/sources.list.d/debian.sources" "$etc_dir/apt/sources.list.d/debian.sources.ci-backup"
fi
cat > "$etc_dir/apt/sources.list" <<'SOURCES'
deb [check-valid-until=no signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://snapshot.debian.org/archive/debian/20260831T000000Z/ bullseye main
deb [check-valid-until=no signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://snapshot.debian.org/archive/debian/20260831T000000Z/ bullseye-updates main
deb [check-valid-until=no signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://snapshot.debian.org/archive/debian-security/20260831T000000Z/ bullseye-security main
SOURCES
