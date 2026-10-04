#!/bin/bash
# Offline manager backup. Database data backups are handled separately.
set -euo pipefail
umask 077
[[ $EUID -eq 0 && $# -eq 2 ]] || { echo "Usage: sudo $0 /absolute/backup.tar.gz.gpg GPG_RECIPIENT"; exit 1; }
root=$(realpath "${HOSTABLE_DATA_DIR:-/etc/hostable}")
output=$(realpath -m "$1")
[[ $output != "$root"/* && ! -e $output ]] || { echo "Choose a new backup file outside the manager data directory."; exit 1; }
command -v gpg >/dev/null
[[ -f $root/.env && -f $root/hostable.db ]] || { echo "This script expects SQLite metadata and .env under HOSTABLE_DATA_DIR. Back up PostgreSQL metadata separately."; exit 1; }
gpg --list-keys "$2" >/dev/null
restart() { if command -v systemctl >/dev/null && [[ -d /run/systemd/system ]]; then systemctl start hostable; else rc-service hostable start; fi; }
if command -v systemctl >/dev/null && [[ -d /run/systemd/system ]]; then systemctl stop hostable; else rc-service hostable stop; fi
trap restart EXIT
# Keeping the manager stopped captures metadata, credentials and certificates together.
tar -C "$root" --exclude=./cache --exclude=./automation --exclude=./backups -czf - . | gpg --batch --encrypt --recipient "$2" --output "$output"
chmod 600 "$output"
echo "Encrypted manager backup saved to $output. Retain database dump files and their separate destination too."
