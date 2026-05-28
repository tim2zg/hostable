#!/bin/bash
set -e

echo "========================================="
echo "   Hostable Native Proxmox Setup         "
echo "========================================="

if ! command -v pveum &> /dev/null; then
    echo "Error: This script must be run directly on a Proxmox host!"
    exit 1
fi

echo "Fetching latest Hostable binary from GitHub..."
wget -qO hostable-linux-amd64 https://github.com/tim2zg/hostable/releases/latest/download/hostable-linux-amd64
chmod +x hostable-linux-amd64

read -p "Enter a VMID for the Hostable Manager LXC (e.g. 999): " vmid
read -p "Enter the Storage ID for the LXC (e.g. local-lvm): " storage_id

echo "Generating Proxmox API Token..."
TOKEN_JSON=$(pveum user token add root@pam hostable --privsep 0 --output-format json 2>/dev/null || true)
if [ -z "$TOKEN_JSON" ]; then
    echo "Token root@pam!hostable might already exist. Regenerating..."
    pveum user token delete root@pam hostable 2>/dev/null || true
    TOKEN_JSON=$(pveum user token add root@pam hostable --privsep 0 --output-format json)
fi

TOKEN_SECRET=$(echo $TOKEN_JSON | grep -o '"value":"[^"]*"' | cut -d'"' -f4)
PVE_HOST="127.0.0.1"

echo "Downloading Alpine template..."
pveam update
pveam download local alpine-3.18-default_20230622_amd64.tar.xz || true

echo "Creating LXC Container $vmid..."
pct create $vmid local:vztmpl/alpine-3.18-default_20230622_amd64.tar.xz -net0 name=eth0,bridge=vmbr0,ip=dhcp -storage $storage_id -unprivileged 1 -features nesting=1
pct set $vmid -onboot 1

echo "Pushing Hostable binary to container..."
pct push $vmid ./hostable-linux-amd64 /usr/local/bin/hostable -perms 755

echo "Creating configuration..."
mkdir -p /tmp/hostable-config

cat > /tmp/hostable-config/hostable.confd << EOF
export PROXMOX_HOST="$PVE_HOST"
export PROXMOX_TOKEN_ID="root@pam!hostable"
export PROXMOX_TOKEN_SECRET="$TOKEN_SECRET"
EOF

cat > /tmp/hostable-config/config.yaml << 'EOF'
interval_seconds: 3600
deployments:
  - image: alpine:latest
    vmid: 103
EOF

cat > /tmp/hostable-config/hostable.init << 'EOF'
#!/sbin/openrc-run
description="Hostable Daemon"
command="/usr/local/bin/hostable"
command_args="manage --config /etc/hostable/config.yaml"
command_background="yes"
pidfile="/run/hostable.pid"
directory="/etc/hostable"
EOF

echo "Pushing configuration to container..."
pct exec $vmid -- mkdir -p /etc/hostable
pct push $vmid /tmp/hostable-config/hostable.confd /etc/conf.d/hostable
pct push $vmid /tmp/hostable-config/config.yaml /etc/hostable/config.yaml
pct push $vmid /tmp/hostable-config/hostable.init /etc/init.d/hostable -perms 755

echo "Setting up OpenRC service..."
pct exec $vmid -- rc-update add hostable default

echo "Starting Hostable LXC Container..."
pct start $vmid

rm -rf /tmp/hostable-config
rm hostable-linux-amd64

echo "Setup complete! The Hostable daemon is now running natively inside LXC $vmid."
echo "You can check its logs by entering the container: pct exec $vmid -- cat /var/log/messages"
