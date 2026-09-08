<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost, apiDelete } from './api';
  import { toasts } from './toast';

  type Route = {
    domain: string;
    target_ip: string;
    target_port: number;
    service_name: string;
    vmid: number;
    mode: string;
    created_at?: string;
  };

  type GatewayStatus = {
    connected: boolean;
    gateway_url: string;
    active_routes: number;
    e2ee_mode: string;
  };

  let status: GatewayStatus = {
    connected: false,
    gateway_url: 'http://127.0.0.1:3000',
    active_routes: 0,
    e2ee_mode: 'Checking...',
  };

  let routes: Route[] = [];
  let isLoading = true;

  // New Route Form
  let showAddModal = false;
  let newDomain = '';
  let newTargetIp = '';
  let newTargetPort = 80;
  let newServiceName = '';
  let newVmid = 100;
  let isSaving = false;

  async function loadData() {
    isLoading = true;
    try {
      const [statusRes, routesRes] = await Promise.all([
        apiGet('/secureweb/status'),
        apiGet('/secureweb/routes'),
      ]);
      status = statusRes;
      routes = routesRes;
    } catch (err: any) {
      toasts.add(`Failed to load SecureWeb status: ${err.message}`, 'error');
    } finally {
      isLoading = false;
    }
  }

  async function handleAddRoute() {
    if (!newDomain || !newTargetIp) {
      toasts.add('Domain and Target IP are required', 'warn');
      return;
    }

    isSaving = true;
    try {
      await apiPost('/secureweb/routes', {
        domain: newDomain,
        target_ip: newTargetIp,
        target_port: Number(newTargetPort),
        service_name: newServiceName || newDomain.split('.')[0],
        vmid: Number(newVmid),
        mode: 'zero-trust',
      });
      toasts.add(`Route for ${newDomain} registered successfully!`, 'success');
      showAddModal = false;
      newDomain = '';
      newTargetIp = '';
      newServiceName = '';
      await loadData();
    } catch (err: any) {
      toasts.add(`Error: ${err.message}`, 'error');
    } finally {
      isSaving = false;
    }
  }

  async function handleDeleteRoute(domain: string) {
    if (!confirm(`Are you sure you want to remove the gateway route for ${domain}?`)) return;

    try {
      await apiDelete(`/secureweb/routes/${encodeURIComponent(domain)}`);
      toasts.add(`Removed route for ${domain}`, 'success');
      await loadData();
    } catch (err: any) {
      toasts.add(`Failed to remove route: ${err.message}`, 'error');
    }
  }

  onMount(() => {
    loadData();
  });
</script>

<div class="secureweb-view">
  <div class="header-section">
    <div>
      <h1 class="page-title">🛡️ SecureWeb Gateway</h1>
      <p class="page-subtitle">
        Zero-Trust Post-Quantum E2EE Gateway integration. Seamlessly exposes Hostable LXCs without custom reverse proxies.
      </p>
    </div>
    <div class="actions">
      <button class="btn btn-secondary" on:click={loadData} disabled={isLoading}>
        🔄 Refresh
      </button>
      <button class="btn btn-primary" on:click={() => showAddModal = true}>
        ➕ Add Ingress Route
      </button>
    </div>
  </div>

  <!-- Gateway Status Card -->
  <div class="status-card">
    <div class="status-item">
      <div class="status-label">Gateway Status</div>
      <div class="status-value">
        {#if status.connected}
          <span class="badge badge-success">🟢 Connected</span>
        {:else}
          <span class="badge badge-warning">🟡 Standalone / Fallback</span>
        {/if}
      </div>
    </div>

    <div class="status-item">
      <div class="status-label">Gateway Endpoint</div>
      <div class="status-value mono">{status.gateway_url}</div>
    </div>

    <div class="status-item">
      <div class="status-label">Encryption Profile</div>
      <div class="status-value">{status.e2ee_mode}</div>
    </div>

    <div class="status-item">
      <div class="status-label">Protected Services</div>
      <div class="status-value bold">{routes.length} Active</div>
    </div>
  </div>

  <!-- Active Routes Section -->
  <div class="routes-section">
    <h2>Registered Services & Upstreams</h2>
    {#if isLoading}
      <div class="loading-state">Loading gateway routes...</div>
    {:else if routes.length === 0}
      <div class="empty-state">
        <p>No services registered with SecureWeb Gateway yet.</p>
        <p class="text-muted">Deploy a container with "Expose via SecureWeb" enabled, or add a route manually.</p>
      </div>
    {:else}
      <div class="table-container">
        <table class="routes-table">
          <thead>
            <tr>
              <th>Service</th>
              <th>Public Domain</th>
              <th>Internal Upstream</th>
              <th>VMID</th>
              <th>Security Mode</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each routes as route}
              <tr>
                <td class="bold">{route.service_name}</td>
                <td>
                  <a href={`https://${route.domain}`} target="_blank" rel="noreferrer" class="domain-link">
                    🌐 {route.domain} ↗
                  </a>
                </td>
                <td class="mono">{route.target_ip}:{route.target_port}</td>
                <td><span class="badge badge-vmid">#{route.vmid}</span></td>
                <td>
                  <span class="badge badge-e2ee">🔒 {route.mode}</span>
                </td>
                <td>
                  <button class="btn-icon danger" title="Remove Route" on:click={() => handleDeleteRoute(route.domain)}>
                    🗑️
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>

  <!-- Modal for manual route -->
  {#if showAddModal}
    <div class="modal-backdrop" on:click|self={() => showAddModal = false}>
      <div class="modal-content">
        <h3>Register New SecureWeb Ingress Route</h3>
        <p class="modal-desc">Routes HTTPS traffic through SecureWeb Gateway's WAF and Zero-Trust E2EE router to your LXC.</p>

        <div class="form-grid">
          <div class="form-group">
            <label for="sw-domain">Public Domain / FQDN</label>
            <input id="sw-domain" type="text" placeholder="e.g. jellyfin.homelab.net" bind:value={newDomain} />
          </div>

          <div class="form-group">
            <label for="sw-ip">Target Container IP</label>
            <input id="sw-ip" type="text" placeholder="e.g. 10.0.1.105" bind:value={newTargetIp} />
          </div>

          <div class="form-row">
            <div class="form-group">
              <label for="sw-port">Port</label>
              <input id="sw-port" type="number" bind:value={newTargetPort} />
            </div>
            <div class="form-group">
              <label for="sw-vmid">VMID</label>
              <input id="sw-vmid" type="number" bind:value={newVmid} />
            </div>
          </div>

          <div class="form-group">
            <label for="sw-service">Service Label</label>
            <input id="sw-service" type="text" placeholder="e.g. Media Server" bind:value={newServiceName} />
          </div>
        </div>

        <div class="modal-actions">
          <button class="btn btn-secondary" on:click={() => showAddModal = false}>Cancel</button>
          <button class="btn btn-primary" on:click={handleAddRoute} disabled={isSaving}>
            {isSaving ? 'Registering...' : 'Register Route'}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .secureweb-view {
    max-width: 1100px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  .header-section {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 2rem;
    flex-wrap: wrap;
    gap: 1rem;
  }

  .page-title {
    font-size: 1.75rem;
    font-weight: 700;
    color: #f8fafc;
    margin-bottom: 0.25rem;
  }

  .page-subtitle {
    color: #94a3b8;
    font-size: 0.95rem;
  }

  .actions {
    display: flex;
    gap: 0.75rem;
  }

  .status-card {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 1.5rem;
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 10px;
    padding: 1.5rem;
    margin-bottom: 2.5rem;
  }

  .status-label {
    font-size: 0.85rem;
    color: #94a3b8;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 0.5rem;
  }

  .status-value {
    font-size: 1.1rem;
    color: #f1f5f9;
  }

  .status-value.mono {
    font-family: monospace;
    font-size: 0.95rem;
    color: #38bdf8;
  }

  .badge {
    display: inline-block;
    padding: 0.25rem 0.6rem;
    border-radius: 9999px;
    font-size: 0.8rem;
    font-weight: 600;
  }

  .badge-success {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .badge-warning {
    background: rgba(234, 179, 8, 0.15);
    color: #facc15;
    border: 1px solid rgba(234, 179, 8, 0.3);
  }

  .badge-vmid {
    background: #0f172a;
    color: #cbd5e1;
    border: 1px solid #334155;
  }

  .badge-e2ee {
    background: rgba(99, 102, 241, 0.15);
    color: #818cf8;
    border: 1px solid rgba(99, 102, 241, 0.3);
  }

  .routes-section h2 {
    font-size: 1.25rem;
    color: #f8fafc;
    margin-bottom: 1rem;
  }

  .table-container {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 10px;
    overflow-x: auto;
  }

  .routes-table {
    width: 100%;
    border-collapse: collapse;
    text-align: left;
  }

  .routes-table th {
    background: #0f172a;
    padding: 0.85rem 1.25rem;
    color: #94a3b8;
    font-size: 0.85rem;
    font-weight: 600;
    text-transform: uppercase;
  }

  .routes-table td {
    padding: 1rem 1.25rem;
    border-top: 1px solid #334155;
    color: #e2e8f0;
    font-size: 0.95rem;
  }

  .domain-link {
    color: #38bdf8;
    text-decoration: none;
    font-weight: 500;
  }

  .domain-link:hover {
    text-decoration: underline;
  }

  .btn {
    padding: 0.6rem 1.2rem;
    border-radius: 6px;
    font-size: 0.95rem;
    font-weight: 600;
    cursor: pointer;
    border: none;
    transition: all 0.2s;
  }

  .btn-primary {
    background: #3b82f6;
    color: #fff;
  }
  .btn-primary:hover {
    background: #2563eb;
  }

  .btn-secondary {
    background: #334155;
    color: #e2e8f0;
  }
  .btn-secondary:hover {
    background: #475569;
  }

  .btn-icon.danger {
    background: transparent;
    border: 1px solid #ef4444;
    color: #ef4444;
    padding: 0.35rem 0.6rem;
    border-radius: 6px;
    cursor: pointer;
  }

  .empty-state {
    padding: 3rem;
    text-align: center;
    background: #1e293b;
    border: 1px dashed #334155;
    border-radius: 10px;
  }

  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 999;
  }

  .modal-content {
    background: #1e293b;
    border: 1px solid #475569;
    border-radius: 12px;
    padding: 2rem;
    width: 100%;
    max-width: 500px;
  }

  .form-group {
    margin-bottom: 1.25rem;
  }

  .form-group label {
    display: block;
    color: #cbd5e1;
    font-size: 0.9rem;
    margin-bottom: 0.4rem;
  }

  .form-group input {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    padding: 0.65rem 0.85rem;
    color: #fff;
    box-sizing: border-box;
  }

  .form-row {
    display: flex;
    gap: 1rem;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.75rem;
    margin-top: 1.5rem;
  }
</style>
