# Hostable

Hostable is an automated deployment manager for Proxmox LXC containers. It seamlessly bridges the gap between Docker OCI registries and native Proxmox LXC virtualization by intelligently syncing Docker container image updates directly into your Proxmox cluster without relying on Docker daemons!

## Features
- **Native LXC Performance**: Run your apps directly on Proxmox LXC containers.
- **Smart Updates**: Automatically detects whether an update only requires an in-place restart or a full container recreation.
- **Automated Deployments**: A simple configuration file maps your Docker images to Proxmox VMIDs.

## Installation

Hostable is designed to run directly on your Proxmox server as a lightweight system daemon. You do not need to compile anything.

To install the Hostable Manager on your Proxmox server, log in to your server as `root` and run:

```bash
wget -qO- https://raw.githubusercontent.com/tim2zg/hostable/main/install.sh | bash
```

### What does the script do?
The installation script will automatically:
1. Generate an isolated Proxmox API token (`root@pam!hostable`).
2. Download the latest Alpine Linux OS template.
3. Spin up an unprivileged LXC container explicitly for the Hostable Manager.
4. Download the latest Hostable release binary and inject it into the container along with a default configuration.
5. Setup the background system daemon.

## Configuration

Once installed, you can configure your deployments by editing the configuration file located inside the Hostable manager container:

1. Open the Proxmox Shell or SSH into your host.
2. Enter the Hostable LXC Container (replace `999` with the VMID you chose during installation):
   ```bash
   pct exec 999 -- vi /etc/hostable/config.yaml
   ```
3. Add your Docker images and target VMIDs:
   ```yaml
   interval_seconds: 3600
   deployments:
     - image: "alpine:latest"
       vmid: 103
     - image: "lscr.io/linuxserver/jellyfin:latest"
       vmid: 104
   ```
4. Restart the Hostable service inside the container to apply changes:
   ```bash
   pct exec 999 -- rc-service hostable restart
   ```

## Development
To compile the Hostable binary yourself on Windows:
1. Ensure Docker Desktop is running.
2. Run `build.bat` in the project root. This uses an Alpine Docker container to cross-compile the Rust backend, outputting `hostable-linux-amd64`.
