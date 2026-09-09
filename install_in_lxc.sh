#!/bin/bash
set -e

echo "========================================="
echo "   Hostable In-Container Setup           "
echo "========================================="

if [ "$EUID" -ne 0 ]; then
  echo "Please run this script as root inside your LXC container."
  exit 1
fi

read -p "Enter your Proxmox Host IP or Domain (e.g. 10.0.1.41): " pve_host
read -p "Enter your Proxmox API Token ID (e.g. root@pam!hostable): " token_id
read -p "Enter your Proxmox API Token Secret: " token_secret

echo "Fetching latest Hostable binary from GitHub..."
wget -qO /usr/local/bin/hostable https://github.com/tim2zg/hostable/releases/latest/download/hostable-linux-amd64
chmod +x /usr/local/bin/hostable

echo "Creating configuration..."
mkdir -p /etc/hostable

cat > /etc/hostable/.env << EOF
PROXMOX_HOST="$pve_host"
PROXMOX_TOKEN_ID="$token_id"
PROXMOX_TOKEN_SECRET="$token_secret"
PORT="3000"
HOSTABLE_DEFAULT_NETWORK="eth0"
DATABASE_URL="sqlite:///etc/hostable/hostable.db?mode=rwc"
EOF

# Detect init system
if command -v systemctl &> /dev/null && [ -d /run/systemd/system ]; then
    echo "Detected systemd. Setting up service..."
    cat > /etc/systemd/system/hostable.service << 'EOF'
[Unit]
Description=Hostable Platform Manager
After=network.target

[Service]
Type=simple
EnvironmentFile=/etc/hostable/.env
ExecStart=/usr/local/bin/hostable start
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now hostable
elif command -v openrc-run &> /dev/null; then
    echo "Detected OpenRC. Setting up service..."
    cat > /etc/init.d/hostable << 'EOF'
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
    chmod +x /etc/init.d/hostable
    rc-update add hostable default
    rc-service hostable restart
else
    echo "Warning: Neither systemd nor OpenRC detected."
    echo "You can run the platform manually:"
    echo "source /etc/hostable/.env && hostable start &"
fi

echo ""
echo "==============================================================="
echo "🎉 HOSTABLE INSTALLED IN CONTAINER!"
echo "==============================================================="
echo "🌐 Access Web Dashboard: http://<container-ip>:3000"
echo "📁 Database: Embedded SQLite (/etc/hostable/hostable.db)"
echo "==============================================================="
