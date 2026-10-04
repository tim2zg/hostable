#!/bin/bash
set -euo pipefail
umask 077
[[ $EUID -eq 0 && $# -eq 2 ]] || { echo "Usage: sudo $0 backup.tar.gz.gpg /absolute/empty/restore-directory"; exit 1; }
[[ $2 == /* ]] || { echo "Use an absolute target path."; exit 1; }
mkdir -p -- "$2"
target=$(realpath "$2")
[[ -z $(find "$target" -mindepth 1 -maxdepth 1 -print -quit) ]] || { echo "Target must be empty; existing metadata is never overwritten."; exit 1; }
chmod 700 "$target"
work=$(mktemp -d); trap 'rm -rf -- "$work"' EXIT
gpg --batch --decrypt "$1" > "$work/manager.tar.gz"
# Validate every archive member before extraction, including link targets.
python3 - "$work/manager.tar.gz" <<'PY'
import pathlib,sys,tarfile
with tarfile.open(sys.argv[1]) as archive:
    for member in archive.getmembers():
        path=pathlib.PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or member.isdev(): raise SystemExit('Unsafe archive member')
        if member.issym() or member.islnk():
            link=pathlib.PurePosixPath(member.linkname)
            if link.is_absolute() or '..' in link.parts: raise SystemExit('Unsafe archive link')
PY
tar -xzf "$work/manager.tar.gz" -C "$target" --no-same-owner
chmod 600 "$target/.env" "$target/admin-token" "$target/hostable.db"
echo "Manager state restored into $target. Install the matching binary, restore database dump files at the configured path, and start this manager after disabling the old one."
