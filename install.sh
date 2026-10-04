#!/bin/bash
set -euo pipefail
umask 077
[[ $EUID -eq 0 ]] && command -v pveum >/dev/null || { echo "Run as root on a Proxmox host."; exit 1; }
read -r -p "Manager VMID [999]: " vmid; vmid=${vmid:-999}
read -r -p "Root storage [local-lvm]: " storage; storage=${storage:-local-lvm}
read -r -p "Template storage [local]: " templates; templates=${templates:-local}
read -r -p "Bridge [vmbr0]: " bridge; bridge=${bridge:-vmbr0}
read -r -p "Proxmox host address reachable from the manager guest: " pve_host
[[ $vmid =~ ^[0-9]+$ && $vmid -ge 100 && $storage =~ ^[A-Za-z0-9_.-]+$ && $templates =~ ^[A-Za-z0-9_.-]+$ && $bridge =~ ^[A-Za-z0-9_.-]+$ && $pve_host =~ ^[A-Za-z0-9.-]+$ ]] || { echo "Invalid settings"; exit 1; }
[[ $pve_host != 127.* && $pve_host != localhost ]] || { echo "Use a host address reachable from the guest."; exit 1; }
if pct config "$vmid" >/dev/null 2>&1 || qm config "$vmid" >/dev/null 2>&1; then
  echo "VMID already exists. Upgrade from inside its guest with install_in_lxc.sh; credentials will be preserved."; exit 1
fi
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
pveam update
template=$(pveam available --section system | awk '$2 ~ /^debian-13-standard_.*amd64\.tar\.(zst|xz|gz)$/ {print $2}' | sort -V | tail -1)
[[ -n $template ]] || { echo "Debian 13 standard template unavailable; select a supported template manually."; exit 1; }
pveam download "$templates" "$template"
privileges="VM.Allocate VM.Audit VM.Backup VM.Clone VM.Config.CPU VM.Config.Disk VM.Config.Memory VM.Config.Network VM.Config.Options VM.Console VM.PowerMgmt VM.Snapshot VM.Snapshot.Rollback Datastore.AllocateTemplate Datastore.AllocateSpace Datastore.Audit SDN.Use"
if pveum role list --output-format json | python3 -c 'import json,sys; sys.exit(not any(r["roleid"]=="HostableDeployer" for r in json.load(sys.stdin)))'; then
  pveum role modify HostableDeployer -privs "$privileges"
else pveum role add HostableDeployer -privs "$privileges"; fi
if ! pveum user list --output-format json | python3 -c 'import json,sys; sys.exit(not any(r["userid"]=="hostable@pve" for r in json.load(sys.stdin)))'; then pveum user add hostable@pve; fi
pveum acl modify / -user hostable@pve -role HostableDeployer
# A new token is created for this manager. Existing tokens are never revoked.
pveum user token add hostable@pve "manager-$vmid" --privsep 0 --output-format json > "$work/token.json"
token_secret=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])' < "$work/token.json")
cat > "$work/env" <<EOF
PROXMOX_HOST=$pve_host
PROXMOX_TOKEN_ID=hostable@pve!manager-$vmid
PROXMOX_TOKEN_SECRET=$token_secret
PROXMOX_CA_CERT=/etc/hostable/proxmox-ca.pem
PROXMOX_INSECURE_TLS=false
HOSTABLE_DATA_DIR=/etc/hostable
HOSTABLE_DEFAULT_BRIDGE=$bridge
DATABASE_URL=sqlite:///etc/hostable/hostable.db?mode=rwc
PORT=3000
EOF
pct create "$vmid" "$templates:vztmpl/$template" --net0 "name=eth0,bridge=$bridge,ip=dhcp" --rootfs "$storage:8" --memory 2048 --cores 2 --unprivileged 1 --features nesting=1 --hostname hostable-manager --onboot 1
pct start "$vmid"
for attempt in {1..60}; do if pct exec "$vmid" -- true >/dev/null 2>&1; then break; fi; sleep 1; done
pct exec "$vmid" -- mkdir -p /etc/hostable
pct exec "$vmid" -- chmod 700 /etc/hostable
pct push "$vmid" "$work/env" /etc/hostable/.env -perms 600
pct push "$vmid" /etc/pve/pve-root-ca.pem /etc/hostable/proxmox-ca.pem -perms 644
installer=$(dirname "$(readlink -f "$0")")/install_in_lxc.sh
[[ -r $installer ]] || { echo "Keep install.sh and install_in_lxc.sh together."; exit 1; }
pct push "$vmid" "$installer" /root/install-hostable.sh -perms 700
pct exec "$vmid" -- bash /root/install-hostable.sh
echo "Manager LXC $vmid installed. Retrieve its token with: pct exec $vmid -- cat /etc/hostable/admin-token"
echo "Verify Proxmox connectivity in the dashboard before deploying workloads."
