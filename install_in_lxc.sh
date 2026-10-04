#!/bin/bash
set -euo pipefail
umask 077
[[ $EUID -eq 0 ]] || { echo "Run as root inside the Hostable manager guest."; exit 1; }
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
if command -v apt-get >/dev/null; then
  apt-get update
  apt-get install -y ca-certificates curl ansible openssh-client postgresql-client openssl xz-utils gnupg python3 util-linux
elif command -v apk >/dev/null; then
  apk add --no-cache ca-certificates curl ansible openssh-client postgresql17-client openssl xz libgcc gnupg python3 util-linux bash
else
  echo "A Debian or Alpine manager guest is required."; exit 1
fi
mkdir -p /etc/hostable
chmod 700 /etc/hostable
if [[ ! -e /etc/hostable/.env ]]; then
  read -r -p "Reachable Proxmox host IP or DNS name: " pve_host
  read -r -p "Proxmox API token ID: " token_id
  read -r -s -p "Proxmox API token secret: " token_secret; echo
  [[ $pve_host =~ ^[A-Za-z0-9.-]+$ && $token_id =~ ^[A-Za-z0-9_.@!-]+$ && $token_secret =~ ^[A-Za-z0-9_-]+$ ]] || { echo "Invalid host or token format."; exit 1; }
  read -r -p "Path to trusted Proxmox CA PEM (must already exist in this guest): " ca_path
  [[ -r $ca_path ]] || { echo "A readable CA certificate is required."; exit 1; }
  cp -- "$ca_path" /etc/hostable/proxmox-ca.pem
  cat > /etc/hostable/.env <<EOF
PROXMOX_HOST=$pve_host
PROXMOX_TOKEN_ID=$token_id
PROXMOX_TOKEN_SECRET=$token_secret
PROXMOX_CA_CERT=/etc/hostable/proxmox-ca.pem
PROXMOX_INSECURE_TLS=false
HOSTABLE_DATA_DIR=/etc/hostable
DATABASE_URL=sqlite:///etc/hostable/hostable.db?mode=rwc
PORT=3000
EOF
  chmod 600 /etc/hostable/.env
else
  echo "Preserving existing configuration and API credentials."
fi
if ! grep -q '^HOSTABLE_SELF_UPDATE_ENABLED=' /etc/hostable/.env; then
  printf '\nHOSTABLE_SELF_UPDATE_ENABLED=true\n' >> /etc/hostable/.env
fi
release=${HOSTABLE_RELEASE_TAG:-latest}
if [[ $release == latest ]]; then base=https://github.com/tim2zg/hostable/releases/latest/download
else
  [[ $release =~ ^v[0-9]+\.[0-9]+\.[0-9]+([A-Za-z0-9.-]*)?$ ]] || { echo "Invalid release tag"; exit 1; }
  base="https://github.com/tim2zg/hostable/releases/download/$release"
fi
curl --fail --location --retry 3 "$base/hostable-linux-amd64" -o "$work/hostable-linux-amd64"
curl --fail --location --retry 3 "$base/hostable-linux-amd64.sha256" -o "$work/checksum"
expected=$(awk 'NR==1 {print $1}' "$work/checksum")
[[ $expected =~ ^[a-fA-F0-9]{64}$ ]] || { echo "Invalid release checksum"; exit 1; }
actual=$(sha256sum "$work/hostable-linux-amd64" | awk '{print $1}')
[[ $expected == "$actual" ]] || { echo "Release checksum mismatch"; exit 1; }
chmod 755 "$work/hostable-linux-amd64"
"$work/hostable-linux-amd64" --help >/dev/null
if [[ -e /usr/local/bin/hostable ]]; then cp -p /usr/local/bin/hostable /usr/local/bin/hostable.previous; fi
install -m 755 "$work/hostable-linux-amd64" /usr/local/bin/hostable.new
mv -f /usr/local/bin/hostable.new /usr/local/bin/hostable
if command -v systemctl >/dev/null && [[ -d /run/systemd/system ]]; then
  cat > /etc/systemd/system/hostable.service <<'EOF'
[Unit]
Description=Hostable Platform Manager
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
WorkingDirectory=/etc/hostable
EnvironmentFile=/etc/hostable/.env
ExecStart=/usr/local/bin/hostable start
Restart=on-failure
RestartSec=5
UMask=0077
[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable hostable
  if ! systemctl restart hostable; then
    [[ ! -e /usr/local/bin/hostable.previous ]] || cp -p /usr/local/bin/hostable.previous /usr/local/bin/hostable
    systemctl restart hostable || true; exit 1
  fi
elif command -v openrc-run >/dev/null; then
  cat > /etc/init.d/hostable <<'EOF'
#!/sbin/openrc-run
description="Hostable Platform Manager"
command="/usr/local/bin/hostable"
command_args="start"
command_background="yes"
pidfile="/run/hostable.pid"
directory="/etc/hostable"
output_log="/var/log/hostable.log"
error_log="/var/log/hostable.log"
depend() { need net; }
EOF
  chmod 755 /etc/init.d/hostable
  touch /var/log/hostable.log; chmod 600 /var/log/hostable.log
  rc-update add hostable default
  rc-service hostable restart
else
  echo "Run: cd /etc/hostable && /usr/local/bin/hostable start"; exit 0
fi
port=$(sed -n 's/^PORT=//p' /etc/hostable/.env | tr -d '\"' | tail -1); port=${port:-3000}
for attempt in {1..20}; do
  if curl --fail --silent "http://127.0.0.1:$port/api/health" >/dev/null; then
    echo "Hostable is running. Admin token: /etc/hostable/admin-token"; exit 0
  fi
  sleep 1
done
echo "Startup check failed; restoring previous binary if available. Inspect service logs."
if [[ -e /usr/local/bin/hostable.previous ]]; then
  cp -p /usr/local/bin/hostable.previous /usr/local/bin/hostable
  if command -v systemctl >/dev/null && [[ -d /run/systemd/system ]]; then systemctl restart hostable; else rc-service hostable restart; fi
fi
exit 1
