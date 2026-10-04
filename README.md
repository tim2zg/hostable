# Hostable

Hostable is a Proxmox LXC container platform with a Rust API and an embedded Svelte dashboard. It can also provision a dedicated PostgreSQL guest so your applications connect directly using normal PostgreSQL clients.

The local implementation includes OCI conversion, managed image replacement and rollback, guest package/script updates, durable operation recovery, health/log views, PostgreSQL hosting, encrypted backups and restore verification. Live Proxmox installation and database acceptance checks are still required before production use. See [BUILD_PLAN.md](BUILD_PLAN.md) for implementation status and remaining release gates.

## Build and run locally

Prerequisites: Rust 1.88, Node 22.21.1, npm, Python 3 and OpenSSL for the local API/TLS registry tests. On Windows, the tests can use OpenSSL bundled with Git for Windows. Linux deployment additionally needs Ansible, SSH and PostgreSQL client tools. Use a pg_dump/pg_restore major version at least as new as the hosted server.

```sh
cd frontend
npm ci
npm run check
npm run build
cd ..
cargo test --locked --manifest-path backend/Cargo.toml
cargo build --locked --manifest-path backend/Cargo.toml
python -m unittest discover -s tests -v
```

Copy `.env.example` to `.env`, configure the reachable Proxmox host, node, scoped API token and trusted cluster CA, then run:

```sh
cargo run --locked --manifest-path backend/Cargo.toml -- start
```

The dashboard defaults to port 3000. The first admin token is saved with restricted permissions in `HOSTABLE_DATA_DIR/admin-token`; the default local data directory is `./hostable-data`. The metadata database defaults to `./hostable.db`. API authentication uses `Authorization: Bearer <token>`; query tokens are accepted only for authenticated WebSocket upgrades. Put the manager behind HTTPS on a trusted management network. Use `HOSTABLE_BIND=127.0.0.1` for a local-only listener.

For a disposable browser preview after building, run `python tests/preview.py`. It creates temporary SQLite metadata, a fake Proxmox HTTP fixture and a clearly identified preview token. Press Enter to stop it and remove its temporary state. It never connects to Proxmox.

Reset a lost admin token with `hostable reset-admin-token` from the manager's configured working directory. This writes a new protected token file and revokes the previous token without deleting platform metadata.

## Install on Proxmox

Keep `install.sh` and `install_in_lxc.sh` together. Run `sudo bash install.sh` on the Proxmox host to create a new Debian 13 manager guest. Choose a host IP/DNS name reachable from that guest. The installer discovers a current standard template, creates a manager-specific API token, copies the cluster CA, and installs a checksum-verified release. Existing VMIDs are rejected.

For an existing manager, run `sudo bash install_in_lxc.sh` inside its guest. Existing configuration and credentials are preserved, the previous binary is retained, and a failed startup check rolls back the binary. The installer needs a published release with both `hostable-linux-amd64` and its `.sha256` file. To build that artifact locally:

```sh
docker build -f backend/Dockerfile -t hostable .
```

Playbooks are embedded in the binary and extracted into the protected data directory. Proxmox TLS verification defaults to enabled. Configure `PROXMOX_CA_CERT`; `PROXMOX_INSECURE_TLS=true` is an explicit opt-out. Console connections use the same CA and TLS policy. Never use a loopback Proxmox address from a separate manager guest.

## Containers

The deploy wizard chooses node, OCI image, separate template/root disk pools, CPU, memory, network, environment and structured managed volumes. VMID zero allocates an unused ID at execution time. Existing VMIDs are never overwritten. `/api/ansible/deploy`, `/api/deploy`, and the single-image compatibility stack route share the same executor and return HTTP 202 with a durable `task_id`.

An example deployment request:

```json
{
  "hostname": "web",
  "image": "nginx:1.28-alpine",
  "vmid": 0,
  "node": "pve",
  "cores": 2,
  "memory": 1024,
  "disk_size": "8G",
  "template_storage": "local",
  "storage_pool": "local-lvm",
  "net_bridge": "vmbr0",
  "ip_address": "dhcp",
  "app_port": 80,
  "mountpoints": [{"storage":"local-lvm","size_gb":8,"container":"/data"}],
  "env_vars": {}
}
```

Success requires successful Proxmox create/start tasks, a confirmed running state, a real guest IP, and the configured TCP or HTTP health check. HTTP checks can require a path and expected status. Jobs are retained in metadata, progress replays after disconnection, and unfinished jobs become `interrupted` after a manager restart. The Jobs view can inspect live ownership, request cancellation at a safe checkpoint, recover a partial deployment, or restore the original container after an interrupted update. Active Proxmox tasks finish before recovery changes their resources. Conflicting mutations are blocked until the operation is resolved.

The OCI contract currently supports Linux amd64, gzip layers and root image users. Images with a non-root `User` are rejected explicitly; images may drop privileges in their own entrypoint. Images need `/bin/sh` and a usable network setup; the fallback init requires `setsid`. Configured systemd/OpenRC init is retained. A bare BusyBox init link gets a managed launcher that starts the image entrypoint and forwards shutdown signals. Layer/config digests and download/extraction bounds are checked. Environment files are root-only inside the guest. Conversion publishes the archive only after compression succeeds and removes its unpacked cache on success or error. Generated templates are removed after successful guest creation; a failed or interrupted create can leave a template which an administrator must inspect and remove.

Registry pulls use verified HTTPS. For a private registry CA, set `HOSTABLE_OCI_CA_CERT` to its PEM certificate on the manager. No insecure registry mode is provided.

The catalog uses immutable JSON recipes with an ID/version, image, required environment keys, persistent volumes, health check and update policy. Import a new version to change a recipe. The Nginx entry is a candidate pending Proxmox smoke testing. Multi-image filesystem merging is disabled. Deploy dependent services separately and wire them through their documented network interfaces.

Lifecycle and snapshot operations resolve the resource's actual node and reject QEMU resources. The LXC Manager exposes console transport. Applications and updates exposes managed revisions, logs and update previews. Legacy `update-lxc` and automatic `manage` recreation remain disabled; use the managed update workflow below.

## Images, replacement and guest updates

The existing converter in `backend/src/oci.rs` extracts an OCI image and generates an LXC template. Deployment and replacement now record the selected Linux amd64 manifest digest, so the reviewed image stays fixed even if its tag moves. A Dockerfile must first be built and pushed to an accessible registry; `hostable deploy --file` accepts the deployment JSON above and submits a real manager job.

Each managed application has a stable workload ID, initially its first VMID. Its active VMID can change after replacement. In **Applications and updates**, choose an image, review the plan, then apply it. A plan expires after one hour and becomes invalid if the active revision, Proxmox configuration, health check or update policy changes.

Image replacement converts the pinned image using the existing converter and prepares a stopped LXC with a new root disk. The cutover stops both guests, transfers managed persistent volumes on the same node, preserves the original net0 configuration/address/MAC, starts the replacement, checks health and updates configured ingress. The previous root disk stays available with boot-at-host-start disabled. A rollback plan transfers current persistent data to the retained previous root and verifies it again. Files outside persistent mounts remain only in their old root disk; schema changes in persistent data require a compatible application or a backup restore.

Stateful updates require a Proxmox storage supporting backups. The manager makes a pre-update `vzdump` backup and retains it. Volume movement also requires storage support for the native Proxmox move operation. Existing snapshots block volume transfers; resolve them with a verified backup before changing image revisions. Writable bind mounts, unrecorded live mounts, custom UID mappings/LXC options, privileged guests and extra network interfaces block automated replacement. Failed or ambiguous transfers preserve the resources and require inspection.

For containers maintained through their own update manager, configure **APT**, **APK**, or an administrator shell script in the workload policy. Hostable takes a stopped snapshot, runs the update inside the existing guest, then restarts the LXC or runs the configured service restart script and checks health. A failed update stops the guest before snapshot rollback. Snapshot support is required; stateful workloads still require the pre-update backup. A nonzero interval opts into scheduled guest updates and requires a health port; image replacement always uses a reviewed plan.

Guest commands and application logs require explicitly configured node SSH endpoints, a dedicated private key, and verified node entries in `HOSTABLE_NODE_KNOWN_HOSTS`. See `.env.example`. The configured account must be able to run `pct` and `timeout` on the node. SSH verifies the node host key, checks the exact container ownership marker, and passes the administrator script through stdin to `pct exec VMID -- /bin/sh -s`. Scripts run as root inside the guest. Hostable does not install a node key or change SSH access automatically.

The CLI uses the same authenticated manager API and protected admin token as the dashboard:

```sh
hostable deploy --file deployment.json
hostable plan-update --workload 120 --image example/app:2 --backup-storage backups --out update-preview.json
# Read the preview and use its plan.id:
hostable apply-update --workload 120 --plan plan_ID
hostable job --id task_ID
hostable recover-job --id task_ID --action inspect
hostable recover-job --id task_ID --action cancel
```

Use `plan-update --method rollback` for a retained revision, `--method recipe` for the saved guest policy, or `--method refresh_credentials` to deliver rotated database credentials using the recorded image digest. An interrupted update supports `recover-job --action rollback`; a partial deployment supports `--action resume`. The inspection response lists valid actions. Remote CLI access requires an HTTPS `HOSTABLE_MANAGER_URL` ending in `/api`; a private manager CA can be supplied with `HOSTABLE_MANAGER_CA_CERT`.

**Health and recovery** stores reports every minute covering application readiness, Proxmox resource metrics, SQL readiness, database connections and sizes, backup history and certificate warnings. Verified node SSH additionally reports mounted filesystem use and application logs. Logs use `hostable-app` in the systemd journal or `/var/log/hostable-application.log` for newly converted OpenRC/fallback guests. File log rotation belongs in the image or guest recipe. Log responses are bounded and redact known deployment secrets; applications should also avoid logging secrets.

## PostgreSQL hosting

1. Download a Debian 12 or 13 standard OS template in Proxmox.
2. Open **Databases**, choose the node/template/storage/bridge, a stable IPv4 address with CIDR, a data volume size and the application networks allowed to connect.
3. Set `HOSTABLE_MANAGER_CIDR` to the manager's reachable address, usually a `/32`. The manager needs SSH and PostgreSQL connectivity to the guest. Guest configuration uses an SSH key generated under `secrets/`, or your configured `HOSTABLE_GUEST_SSH_KEY` with its `.pub` companion.
4. Wait for the provisioning job to verify a real TLS SQL connection. Create a logical application database and its dedicated role.
5. Reveal connection details, save the trusted certificate, and connect your application with `sslmode=verify-full` and `sslrootcert` pointing to the downloaded certificate.

The guest installs PostgreSQL from its Debian repositories (15 on Debian 12; 17 on Debian 13). Data is initialized only on an empty managed mount at `/var/lib/hostable-postgres/data`. A mismatched PostgreSQL major version is rejected. PostgreSQL uses SCRAM passwords and TLS, and `pg_hba.conf` rejects clients outside the configured networks. The control identity is restricted to the manager CIDR. Each application owns its database; public CONNECT is revoked. The manager's control credential is never returned as an application credential.

For example, using libpq clients:

```sh
psql 'postgresql://APP_USER:APP_PASSWORD@DATABASE_IP:5432/APP_DB?sslmode=verify-full&sslrootcert=/path/to/hostable-database.crt'
```

The UI supplies a correctly escaped URI. Adapt TLS configuration to your application driver; it may use separate certificate/options fields. Your application owns its schema and migrations. Hostable does not generate a CRUD API for application data.

Application deployments can select a hosted application database. Hostable supplies `DATABASE_URL` and `/etc/hostable/database-ca.crt`. Include the application container's network in the allowed CIDRs. After password rotation, use **Refresh attached database credentials** in Applications and updates to deliver the new secret through a managed replacement with the same image digest. Health reports identify attachments needing refresh; external clients must receive the new password separately. Revoke login prevents new connections and terminates existing sessions; rotation enables login again.

Database guest SSH host keys use first-use acceptance and are then pinned per instance. To require pre-pinned keys on the first connection, configure `HOSTABLE_GUEST_KNOWN_HOSTS`. New instances use a stable manager-held CA and 365-day server certificates. The scheduler checks every 15 minutes and renews certificates with fewer than 30 days remaining, reloads PostgreSQL, then verifies a fresh TLS SQL connection. The Databases view also exposes manual renewal. Clients keep the same CA trust. Protect and back up the CA private key; legacy self-signed instances retain their existing trust and need an explicit migration to managed CA identities. Applying access changes restarts the guest database service.

Managed PostgreSQL guests reject generic image/package updates. For a PostgreSQL major migration, create a new supported database instance, quiesce application writes, make a final logical backup and restore to the new instance. Verify the application and networks, then select the new logical database attachment in an application image replacement plan. Retain the source instance and backups for recovery. In-place PostgreSQL major upgrades are not implemented.

## Backups and recovery

Set `HOSTABLE_BACKUP_DIR` to a separate mounted backup destination. The default is `HOSTABLE_DATA_DIR/backups`, which provides local recovery but is not an independent disaster-recovery destination. Configure interval and retention in Databases; zero interval disables scheduling.

Set `HOSTABLE_BACKUP_GPG_RECIPIENT` to a trusted recipient fingerprint in the manager service account's GPG keyring to encrypt hosted database dumps. The backup destination receives ciphertext; plaintext is staged in the protected manager cache and removed after the operation. Encryption failures fail the backup. Restores and automatic verification need batch decryption access to the matching private key. Preserve recovery keys independently and check the recovery report's destination/encryption status.

Backups use PostgreSQL custom-format logical dumps, record SHA-256, and record table row counts from the same exported snapshot. Restore checks archive integrity, creates a new database/role without overwriting existing data, restores with `--exit-on-error`, and compares restored row counts. A failed restore retains its target for inspection and does not publish it as ready. Restore into a separate instance and verify application behavior before switching clients. Logical dumps do not provide point-in-time recovery.

In **Health and recovery**, select a separate ready PostgreSQL instance and run a backup verification now or set a drill interval. Scheduled drills restore the latest backup per application into a generated database/role, compare row counts, and delete only that generated target after success. Failed or interrupted targets stay available for inspection, with their references shown in the recovery view. Verification does not switch application traffic.

Retiring a database instance stops its guest and scheduled backups while preserving the entire LXC, data volume and backup records. Permanent data destruction is intentionally an operator action in Proxmox after verified recovery; the dashboard does not expose a destructive delete path.

Back up manager metadata, tokens, SSH keys, database credentials and certificates together. For the standard SQLite layout under `/etc/hostable`, the provided scripts (run from a repository checkout in the manager guest, with Python 3 and GPG installed) stop the manager during backup, encrypt the archive to a GPG recipient, and restore only into an empty directory:

```sh
sudo bash scripts/backup-manager.sh /mnt/recovery/manager.tar.gz.gpg GPG_RECIPIENT
sudo bash scripts/restore-manager.sh /mnt/recovery/manager.tar.gz.gpg /srv/restored-hostable
```

Use these at an idle maintenance point; stopping the manager interrupts active operations. Retain the separate PostgreSQL dump destination too. If platform metadata uses PostgreSQL or resides outside the standard directory, back up that metadata separately and keep the protected manager directory from the same maintenance point. Disable the old manager before starting a restored one against the same guests.

## Portal releases and updates

Every push to `main` runs the full validation workflow, then builds and publishes the Linux amd64 manager with its embedded dashboard and a SHA-256 checksum. Automatic release tags use the Cargo major/minor version and the Release workflow run number, such as `v1.5.20`. Stable version tags and manual runs on `main` also build releases. The binary reports its release version and source commit through `--version` and `/api/health`.

Use **Settings → Portal updates** to check releases, install a reviewed version, or enable automatic installation. Installed managers check every six hours by default; automatic installation is opt-in. `install_in_lxc.sh` enables the service updater and installs Python and the required process tools. Existing installations need to run that installer once to obtain this functionality; configuration and credentials are preserved. Source and Docker deployments update through their deployment tools.

Self-updates support Linux amd64 systemd/OpenRC services installed at `/usr/local/bin/hostable`, with SQLite metadata inside the manager data directory. The updater validates stable versions, repository asset URLs, checksum, ELF architecture and the binary's reported version. It waits for active jobs, pauses new operations and API changes, then launches a separate worker to stop the service, save the binary and a consistent metadata/configuration backup, install the candidate and check its running version and authenticated readiness. Failed readiness restores the previous binary, metadata, credentials and certificates. PostgreSQL platform metadata requires a manual upgrade with its own backup procedure.

Private update files, previous binaries and local metadata/configuration backups remain under `HOSTABLE_DATA_DIR/updates`. Keep the independent encrypted disaster-recovery backups too; these local snapshots are protected recovery points and should be removed only after accepting an upgrade. A failed version is not automatically retried; an administrator can retry it manually or wait for a newer release.

An interrupted worker keeps changes paused for inspection. If the worker is no longer running, an administrator can recover the recorded pending operation with the installed worker and the staging directory named in `updates/operation.json`:

```sh
sudo python3 /etc/hostable/updates/worker.py recover /etc/hostable/updates/update_OPERATION_ID/request.json
```

Replace `update_OPERATION_ID` with that operation's actual directory. Recovery restores its previous installation when a cutover backup exists and prevents a pending worker from later applying the cancelled operation. First prove service update and rollback on a disposable manager before opting into unattended installation.

## Optional gateway

Set `SECUREWEB_GATEWAY_URL` and, if required, `SECUREWEB_GATEWAY_TOKEN` for an external gateway implementing GET `/api/health`, GET/POST `/api/routes` and DELETE `/api/routes/{domain}`. No fallback gateway is created. Upstream failures are returned, desired and confirmed routes are persisted, and disconnected state is visible. HTTPS certificates, DNS, firewalling and external routing must be configured and verified on that gateway; Hostable does not assert end-to-end or post-quantum encryption from a health response.

## Verification and release gates

CI runs frontend checks/build, backend formatting/unit tests, local HTTP acceptance tests, installer shell syntax and database/certificate playbook syntax. Explicit tests cover GPG encryption/decryption and stable CA signing. The workflow also starts separate disposable TLS PostgreSQL 16/17 servers for role isolation, password rotation, logical backup/restore and cross-instance restore verification. These infrastructure tests are ignored in ordinary unit runs and must be invoked explicitly with their disposable fixtures.

Local checks passed 41 standard backend tests, 38 API tests, eight update-worker tests, explicit encryption and CA tests, frontend type checks and the production build. The API suite uses a disposable HTTPS OCI registry to exercise actual layer conversion, archive upload, reviewed-digest replacement and persistent-volume rollback. It checks private CA trust, corrupt-layer rejection, error cleanup and repeated pulls. Other tests cover failed-start rollback, interrupted cutover recovery, cancellation, stale plans and a disposable guest-command transport. Registry images are protocol fixtures and are never booted. These checks do not establish Linux guest boot, real SSH, UID mappings, gateway traffic or Proxmox API compatibility; the Linux merged-/usr symlink check awaits CI. Live replacement, package/script updates, database provisioning, restore drills, certificate renewal, host restart and installer rollback remain acceptance gates. PITR/WAL archiving is future scope. The build plan tracks these separately from source implementation.
