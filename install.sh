#!/bin/bash
set -e

echo "========================================="
echo "   Hostable Platform Setup (Proxmox)     "
echo "========================================="

if ! command -v pveum &> /dev/null; then
    echo "Error: This script must be run directly on a Proxmox host as root!"
    exit 1
fi

echo "Fetching latest Hostable binary from GitHub..."
wget -qO hostable-linux-amd64 https://github.com/tim2zg/hostable/releases/latest/download/hostable-linux-amd64 || {
    echo "Warning: No pre-compiled binary available yet from releases. Using local binary if present."
}
if [ -f hostable-linux-amd64 ]; then
    chmod +x hostable-linux-amd64
fi

read -p "Enter a VMID for the Hostable Manager LXC [default: 999]: " vmid
vmid=${vmid:-999}

read -p "Enter the Storage Pool for the LXC [default: local-lvm]: " storage_id
storage_id=${storage_id:-local-lvm}

read -p "Enter Default Network Bridge [default: vmbr0]: " net_bridge
net_bridge=${net_bridge:-vmbr0}

read -p "Enter SecureWeb Gateway URL (optional, e.g. http://10.0.1.50:8080): " secureweb_url

echo "Generating Proxmox API Token..."
TOKEN_JSON=$(pveum user token add root@pam hostable --privsep 0 --output-format json 2>/dev/null || true)
if [ -z "$TOKEN_JSON" ]; then
    echo "Token root@pam!hostable might already exist. Regenerating..."
    pveum user token delete root@pam hostable 2>/dev/null || true
    TOKEN_JSON=$(pveum user token add root@pam hostable --privsep 0 --output-format json)
fi

TOKEN_SECRET=$(echo "$TOKEN_JSON" | grep -o '"value":"[^"]*"' | cut -d'"' -f4)
PVE_HOST="127.0.0.1"

echo "Downloading Alpine template..."
pveam update
pveam download local alpine-3.20-default_20240606_amd64.tar.xz 2>/dev/null || pveam download local alpine-3.18-default_20230622_amd64.tar.xz || true

TEMPLATE=$(pveam list local | grep -E "alpine.*default.*amd64\.tar\.xz" | tail -n 1 | awk '{print $1}')
if [ -z "$TEMPLATE" ]; then
    TEMPLATE="local:vztmpl/alpine-3.20-default_20240606_amd64.tar.xz"
fi

echo "Creating LXC Container $vmid..."
pct create "$vmid" "$TEMPLATE" \
    -net0 "name=eth0,bridge=$net_bridge,ip=dhcp" \
    -storage "$storage_id" \
    -memory 2048 \
    -cores 2 \
    -unprivileged 1 \
    -features nesting=1 \
    -hostname "hostable-manager"

pct set "$vmid" -onboot 1

echo "Booting LXC Container for package setup..."
pct start "$vmid"
sleep 5

echo "Installing runtime dependencies (Ansible, OpenRC, CA-Certs)..."
pct exec "$vmid" -- apk update
pct exec "$vmid" -- apk add --no-cache ansible python3 ca-certificates curl tar xz

if [ -f ./hostable-linux-amd64 ]; then
    echo "Pushing Hostable binary to container..."
    pct push "$vmid" ./hostable-linux-amd64 /usr/local/bin/hostable -perms 755
fi

echo "Creating configuration..."
pct exec "$vmid" -- mkdir -p /etc/hostable /etc/conf.d

cat > /tmp/hostable.env << EOF
PROXMOX_HOST="$PVE_HOST"
PROXMOX_TOKEN_ID="root@pam!hostable"
PROXMOX_TOKEN_SECRET="$TOKEN_SECRET"
PORT="3000"
HOSTABLE_DEFAULT_NETWORK="eth0"
HOSTABLE_DEFAULT_BRIDGE="$net_bridge"
SECUREWEB_GATEWAY_URL="$secureweb_url"
DATABASE_URL="sqlite:///etc/hostable/hostable.db?mode=rwc"
EOF

pct push "$vmid" /tmp/hostable.env /etc/hostable/.env -perms 600

cat > /tmp/hostable.init << 'EOF'
#!/sbin/openrc-run
description="Hostable Platform Manager"
command="/usr/local/bin/hostable"
command_args="start"
command_background="yes"
pidfile="/run/hostable.pid"
directory="/etc/hostable"

depend() {
    need net
    after firewall
}
EOF

pct push "$vmid" /tmp/hostable.init /etc/init.d/hostable -perms 755

echo "Enabling and starting Hostable service..."
pct exec "$vmid" -- rc-update add hostable default
pct exec "$vmid" -- rc-service hostable restart

sleep 3
CONTAINER_IP=$(pct exec "$vmid" -- ip -4 addr show eth0 | grep -oP '(?<=inet\s)\d+(\.\d+){3}' | head -n 1 || echo "DHCP Pending")

rm -f /tmp/hostable.env /tmp/hostable.init ./hostable-linux-amd64

echo ""
echo "==============================================================="
echo "🎉 HOSTABLE PLATFORM DEPLOYED SUCCESSFULLY!"
echo "==============================================================="
echo "🌐 Web Dashboard: http://${CONTAINER_IP}:3000"
echo "🔐 Container VMID: $vmid"
echo "📁 Database: Embedded SQLite (/etc/hostable/hostable.db)"
echo "🤖 Invisible Ansible Engine: Ready (/usr/bin/ansible)"
echo ""
echo "To view your Admin API Token, run:"
echo "pct exec $vmid -- grep -E 'Admin API Token' /var/log/messages || pct exec $vmid -- cat /etc/hostable/.env"
echo "==============================================================="

