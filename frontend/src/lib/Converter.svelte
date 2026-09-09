<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet, apiPost } from './api';
  import { toasts } from './toast';

  // Basic container settings
  let image = '';
  let hostname = '';
  let targetVmid = 100;

  // Sliders
  let cores = 2;
  let memoryMb = 2048;
  let diskGb = 16;

  // Discovered infrastructure
  let storagePools: string[] = ['local-zfs', 'local-lvm', 'local'];
  let selectedStorage = 'local-zfs';
  let networkBridges: string[] = ['vmbr0'];
  let selectedBridge = 'vmbr0';
  let defaultNetworkIface = 'eth0';
  let ipMode: 'dhcp' | 'static' = 'dhcp';
  let staticIp = '';
  let gateway = '';

  // Persistent Volumes & Envs
  let mountpoints: { host: string; container: string }[] = [];
  let envVars: { key: string; value: string }[] = [];

  // SecureWeb Gateway
  let exposeSecureWeb = false;
  let secureWebDomain = '';
  let appPort = 80;

  // Live Task Streaming State
  let isDeploying = false;
  let currentTaskId = '';
  let taskEvents: { timestamp: string; level: string; step: string; message: string }[] = [];
  let ws: WebSocket | null = null;
  let deploymentFinished = false;
  let deploymentSuccess = false;

  onMount(async () => {
    // Auto-discover infrastructure from Proxmox
    try {
      const vmidRes = await apiGet('/node/next-vmid');
      if (vmidRes?.next_vmid) targetVmid = vmidRes.next_vmid;
    } catch (e) {
      targetVmid = Math.floor(Math.random() * (900 - 200 + 1) + 200);
    }

    try {
      const netRes = await apiGet('/node/default-network');
      if (netRes?.default_interface) defaultNetworkIface = netRes.default_interface;
      if (netRes?.default_bridge) selectedBridge = netRes.default_bridge;
    } catch (e) {}

    try {
      const storagesRes = await apiGet('/node/storages');
      if (storagesRes?.data && Array.isArray(storagesRes.data)) {
        const poolNames = storagesRes.data
          .filter((s: any) => s.content?.includes('rootdir') || s.type === 'zfspool' || s.type === 'lvmthin')
          .map((s: any) => s.storage);
        if (poolNames.length > 0) {
          storagePools = poolNames;
          selectedStorage = poolNames[0];
        }
      }
    } catch (e) {}

    try {
      const bridgesRes = await apiGet('/node/bridges');
      if (Array.isArray(bridgesRes) && bridgesRes.length > 0) {
        networkBridges = bridgesRes;
        if (!networkBridges.includes(selectedBridge)) {
          selectedBridge = bridgesRes[0];
        }
      }
    } catch (e) {}
  });

  onDestroy(() => {
    if (ws) ws.close();
  });

  function formatMemory(mb: number): string {
    if (mb >= 1024) {
      return `${(mb / 1024).toFixed(mb % 1024 === 0 ? 0 : 1)} GB`;
    }
    return `${mb} MB`;
  }

  function addVolume() {
    mountpoints = [...mountpoints, { host: '', container: '' }];
  }

  function removeVolume(idx: number) {
    mountpoints = mountpoints.filter((_, i) => i !== idx);
  }

  function addEnv() {
    envVars = [...envVars, { key: '', value: '' }];
  }

  function removeEnv(idx: number) {
    envVars = envVars.filter((_, i) => i !== idx);
  }

  async function triggerDeployment() {
    if (!image.trim() || !hostname.trim() || !targetVmid) {
      toasts.add('Please enter an OCI image, hostname, and VMID', 'warn');
      return;
    }

    isDeploying = true;
    deploymentFinished = false;
    deploymentSuccess = false;
    taskEvents = [];

    const envMap: Record<string, string> = {};
    for (const item of envVars) {
      if (item.key.trim()) envMap[item.key.trim()] = item.value;
    }

    const payload = {
      vmid: Number(targetVmid),
      hostname: hostname.trim().toLowerCase().replace(/[^a-z0-9-]/g, '-'),
      image: image.trim(),
      cores,
      memory: memoryMb,
      disk_size: `${diskGb}G`,
      storage_pool: selectedStorage,
      net_bridge: selectedBridge,
      default_network: defaultNetworkIface.trim(),
      ip_address: ipMode === 'dhcp' ? 'dhcp' : staticIp,
      gateway: ipMode === 'static' && gateway ? gateway : null,
      mountpoints: mountpoints.filter(m => m.host && m.container),
      env_vars: envMap,
      expose_secureweb: exposeSecureWeb,
      secureweb_domain: exposeSecureWeb ? secureWebDomain.trim() : null,
      app_port: Number(appPort),
    };

    try {
      const res = await apiPost('/ansible/deploy', payload);
      currentTaskId = res.task_id;
      connectTaskWs(currentTaskId);
    } catch (err: any) {
      toasts.add(`Failed to launch deployment: ${err.message}`, 'error');
      isDeploying = false;
    }
  }

  function connectTaskWs(taskId: string) {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/api/ws/tasks/${taskId}`;
    ws = new WebSocket(wsUrl);

    ws.onmessage = (event) => {
      try {
        const ev = JSON.parse(event.data);
        taskEvents = [...taskEvents, ev];

        if (ev.step === 'COMPLETE') {
          deploymentFinished = true;
          deploymentSuccess = true;
          toasts.add(`Deployment complete for ${hostname}!`, 'success');
        } else if (ev.level === 'error') {
          deploymentFinished = true;
          deploymentSuccess = false;
          toasts.add(`Task error: ${ev.message}`, 'error');
        }
      } catch (e) {}
    };

    ws.onerror = () => {
      toasts.add('WebSocket connection to task runner failed.', 'warn');
    };
  }

  function closeDeployModal() {
    isDeploying = false;
    if (ws) {
      ws.close();
      ws = null;
    }
  }
</script>

<div class="deploy-wizard">
  <div class="wizard-header">
    <h1>🚀 Deploy Container (Invisible Ansible Engine)</h1>
    <p class="subtitle">
      Automated OCI-to-LXC provisioning with persistent dataset mountpoints and SecureWeb Gateway ingress.
    </p>
  </div>

  <div class="card-grid">
    <!-- Step 1: Base Image & Identification -->
    <div class="wizard-card">
      <div class="card-title">
        <span class="step-num">1</span>
        <h3>Image & Identification</h3>
      </div>

      <div class="form-group">
        <label for="input-image">OCI / Docker Image Reference</label>
        <input
          id="input-image"
          type="text"
          placeholder="e.g. alpine:latest, lscr.io/linuxserver/jellyfin:latest"
          bind:value={image}
        />
      </div>

      <div class="form-row">
        <div class="form-group">
          <label for="input-hostname">Container Hostname</label>
          <input
            id="input-hostname"
            type="text"
            placeholder="e.g. jellyfin-app"
            bind:value={hostname}
          />
        </div>
        <div class="form-group">
          <label for="input-vmid">Proxmox VMID</label>
          <input
            id="input-vmid"
            type="number"
            bind:value={targetVmid}
          />
        </div>
      </div>
    </div>

    <!-- Step 2: Resource Allocation Sliders -->
    <div class="wizard-card">
      <div class="card-title">
        <span class="step-num">2</span>
        <h3>Resource Allocation</h3>
      </div>

      <!-- CPU Cores Slider -->
      <div class="slider-group">
        <div class="slider-header">
          <span>CPU Cores</span>
          <span class="slider-value">{cores} Cores</span>
        </div>
        <input type="range" min="1" max="16" step="1" bind:value={cores} />
        <div class="presets">
          <button type="button" on:click={() => cores = 1}>1 Core</button>
          <button type="button" class="recommended" on:click={() => cores = 2}>2 Cores (Default)</button>
          <button type="button" on:click={() => cores = 4}>4 Cores</button>
          <button type="button" on:click={() => cores = 8}>8 Cores</button>
        </div>
      </div>

      <!-- RAM Memory Slider -->
      <div class="slider-group">
        <div class="slider-header">
          <span>RAM Memory</span>
          <span class="slider-value">{formatMemory(memoryMb)}</span>
        </div>
        <input type="range" min="512" max="32768" step="512" bind:value={memoryMb} />
        <div class="presets">
          <button type="button" on:click={() => memoryMb = 1024}>1 GB</button>
          <button type="button" class="recommended" on:click={() => memoryMb = 2048}>2 GB</button>
          <button type="button" on:click={() => memoryMb = 4096}>4 GB</button>
          <button type="button" on:click={() => memoryMb = 8192}>8 GB</button>
        </div>
      </div>

      <!-- Rootfs Disk Size Slider -->
      <div class="slider-group">
        <div class="slider-header">
          <span>Rootfs Storage Disk</span>
          <span class="slider-value">{diskGb} GB</span>
        </div>
        <input type="range" min="4" max="250" step="2" bind:value={diskGb} />
      </div>

      <div class="form-row">
        <div class="form-group">
          <label for="select-storage">Storage Pool</label>
          <select id="select-storage" bind:value={selectedStorage}>
            {#each storagePools as pool}
              <option value={pool}>{pool}</option>
            {/each}
          </select>
        </div>
        <div class="form-group">
          <label for="select-bridge">Network Bridge</label>
          <select id="select-bridge" bind:value={selectedBridge}>
            {#each networkBridges as bridge}
              <option value={bridge}>{bridge}</option>
            {/each}
          </select>
        </div>
        <div class="form-group">
          <label for="input-iface">Default Network Interface</label>
          <input
            id="input-iface"
            type="text"
            placeholder="eth0"
            bind:value={defaultNetworkIface}
          />
        </div>
      </div>
    </div>

    <!-- Step 3: Persistent Volumes (Zero Data Loss) -->
    <div class="wizard-card">
      <div class="card-title">
        <span class="step-num">3</span>
        <h3>Persistent Storage Mountpoints (Zero Data Loss)</h3>
      </div>
      <p class="section-desc">
        Data mounted on external host paths or ZFS datasets persists across container rootfs updates!
      </p>

      {#each mountpoints as vol, idx}
        <div class="dyn-row">
          <input type="text" placeholder="Host Path (e.g. /mnt/data)" bind:value={vol.host} />
          <span class="arrow">➔</span>
          <input type="text" placeholder="Container Path (e.g. /data)" bind:value={vol.container} />
          <button type="button" class="btn-del" on:click={() => removeVolume(idx)}>✕</button>
        </div>
      {/each}

      <button type="button" class="btn-add" on:click={addVolume}>
        ➕ Add Persistent Mountpoint
      </button>
    </div>

    <!-- Step 4: SecureWeb Gateway Integration -->
    <div class="wizard-card highlight">
      <div class="card-title">
        <span class="step-num">4</span>
        <h3>🛡️ SecureWeb Gateway Ingress</h3>
      </div>

      <div class="toggle-row">
        <label class="switch">
          <input type="checkbox" bind:checked={exposeSecureWeb} />
          <span class="slider-toggle"></span>
        </label>
        <span class="toggle-text">Expose via SecureWeb Gateway (Post-Quantum Zero-Trust E2EE)</span>
      </div>

      {#if exposeSecureWeb}
        <div class="secureweb-inputs">
          <div class="form-row">
            <div class="form-group flex-2">
              <label for="input-domain">Public Domain / FQDN</label>
              <input
                id="input-domain"
                type="text"
                placeholder="e.g. jellyfin.homelab.net"
                bind:value={secureWebDomain}
              />
            </div>
            <div class="form-group flex-1">
              <label for="input-port">App Port</label>
              <input
                id="input-port"
                type="number"
                placeholder="80"
                bind:value={appPort}
              />
            </div>
          </div>
          <p class="hint">
            SecureWeb will automatically register this container as a protected upstream with ACME TLS & WAF protection.
          </p>
        </div>
      {/if}
    </div>
  </div>

  <!-- Deploy Action Bar -->
  <div class="action-bar">
    <button class="btn-deploy" on:click={triggerDeployment}>
      🚀 Deploy with Hostable (Invisible Ansible Engine)
    </button>
  </div>

  <!-- Real-time Live Task Streaming Modal -->
  {#if isDeploying}
    <div class="stream-backdrop">
      <div class="stream-modal">
        <div class="stream-header">
          <div>
            <h3>Provisioning Container #{targetVmid}</h3>
            <span class="task-id mono">{currentTaskId}</span>
          </div>
          {#if deploymentFinished}
            <button class="btn-close" on:click={closeDeployModal}>✕</button>
          {/if}
        </div>

        <div class="terminal-view">
          {#each taskEvents as ev}
            <div class="terminal-line {ev.level}">
              <span class="time">{ev.timestamp}</span>
              <span class="tag">[{ev.step}]</span>
              <span class="msg">{ev.message}</span>
            </div>
          {/each}

          {#if !deploymentFinished}
            <div class="terminal-line info pulse">
              <span class="time">...</span>
              <span class="tag">[RUNNING]</span>
              <span class="msg">Executing automation playbook in background...</span>
            </div>
          {/if}
        </div>

        <div class="stream-footer">
          {#if deploymentFinished}
            <button class="btn-primary" on:click={closeDeployModal}>
              {deploymentSuccess ? 'Done' : 'Dismiss'}
            </button>
          {:else}
            <span class="spinner-text">Ansible engine working in background...</span>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .deploy-wizard {
    max-width: 1100px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  .wizard-header {
    margin-bottom: 2rem;
  }

  .wizard-header h1 {
    font-size: 1.85rem;
    color: #f8fafc;
    margin-bottom: 0.35rem;
  }

  .subtitle {
    color: #94a3b8;
    font-size: 1rem;
  }

  .card-grid {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    margin-bottom: 2rem;
  }

  .wizard-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 1.75rem;
  }

  .wizard-card.highlight {
    border-color: #6366f1;
    background: rgba(99, 102, 241, 0.04);
  }

  .card-title {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.25rem;
  }

  .step-num {
    background: #3b82f6;
    color: white;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 0.9rem;
  }

  .card-title h3 {
    margin: 0;
    font-size: 1.25rem;
    color: #f1f5f9;
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

  .form-group input,
  .form-group select {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    padding: 0.75rem 0.9rem;
    color: #f8fafc;
    font-size: 0.95rem;
    box-sizing: border-box;
  }

  .form-row {
    display: flex;
    gap: 1.25rem;
  }

  .form-row > * {
    flex: 1;
  }

  .flex-2 { flex: 2; }
  .flex-1 { flex: 1; }

  /* Sliders */
  .slider-group {
    margin-bottom: 1.5rem;
    background: #0f172a;
    padding: 1rem 1.25rem;
    border-radius: 8px;
    border: 1px solid #334155;
  }

  .slider-header {
    display: flex;
    justify-content: space-between;
    margin-bottom: 0.6rem;
    color: #cbd5e1;
    font-weight: 500;
  }

  .slider-value {
    color: #38bdf8;
    font-weight: 700;
  }

  input[type="range"] {
    width: 100%;
    accent-color: #3b82f6;
  }

  .presets {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }

  .presets button {
    background: #1e293b;
    border: 1px solid #334155;
    color: #cbd5e1;
    padding: 0.25rem 0.6rem;
    border-radius: 4px;
    font-size: 0.8rem;
    cursor: pointer;
  }

  .presets button.recommended {
    border-color: #3b82f6;
    color: #60a5fa;
  }

  /* Mount points */
  .dyn-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
  }

  .dyn-row input {
    flex: 1;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    padding: 0.65rem 0.8rem;
    color: #fff;
  }

  .arrow {
    color: #64748b;
  }

  .btn-del {
    background: transparent;
    border: 1px solid #ef4444;
    color: #ef4444;
    padding: 0.5rem 0.75rem;
    border-radius: 6px;
    cursor: pointer;
  }

  .btn-add {
    background: #334155;
    border: none;
    color: #e2e8f0;
    padding: 0.6rem 1rem;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.9rem;
  }

  /* Toggle */
  .toggle-row {
    display: flex;
    align-items: center;
    gap: 1rem;
  }

  .switch {
    position: relative;
    display: inline-block;
    width: 48px;
    height: 26px;
  }

  .switch input { opacity: 0; width: 0; height: 0; }

  .slider-toggle {
    position: absolute;
    cursor: pointer;
    top: 0; left: 0; right: 0; bottom: 0;
    background-color: #334155;
    transition: 0.3s;
    border-radius: 34px;
  }

  .slider-toggle:before {
    position: absolute;
    content: "";
    height: 18px; width: 18px;
    left: 4px; bottom: 4px;
    background-color: white;
    transition: 0.3s;
    border-radius: 50%;
  }

  input:checked + .slider-toggle {
    background-color: #6366f1;
  }

  input:checked + .slider-toggle:before {
    transform: translateX(22px);
  }

  .toggle-text {
    color: #f1f5f9;
    font-weight: 500;
  }

  .secureweb-inputs {
    margin-top: 1.25rem;
    padding-top: 1.25rem;
    border-top: 1px solid #334155;
  }

  .hint {
    color: #94a3b8;
    font-size: 0.85rem;
    margin-top: 0.5rem;
  }

  /* Deploy Button */
  .action-bar {
    display: flex;
    justify-content: flex-end;
  }

  .btn-deploy {
    background: linear-gradient(135deg, #3b82f6, #6366f1);
    color: white;
    font-size: 1.1rem;
    font-weight: 700;
    padding: 1rem 2.5rem;
    border-radius: 8px;
    border: none;
    cursor: pointer;
    box-shadow: 0 4px 14px rgba(59, 130, 246, 0.4);
    transition: all 0.2s;
  }

  .btn-deploy:hover {
    transform: translateY(-2px);
    box-shadow: 0 6px 20px rgba(59, 130, 246, 0.6);
  }

  /* Task streaming modal */
  .stream-backdrop {
    position: fixed;
    top: 0; left: 0; width: 100%; height: 100%;
    background: rgba(0, 0, 0, 0.8);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 1000;
  }

  .stream-modal {
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 12px;
    width: 90%;
    max-width: 800px;
    padding: 1.5rem;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  }

  .stream-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1rem;
    border-bottom: 1px solid #334155;
    padding-bottom: 0.75rem;
  }

  .stream-header h3 {
    margin: 0;
    color: #f8fafc;
  }

  .task-id {
    font-size: 0.8rem;
    color: #64748b;
  }

  .terminal-view {
    background: #020617;
    border: 1px solid #1e293b;
    border-radius: 8px;
    padding: 1rem;
    height: 350px;
    overflow-y: auto;
    font-family: 'Courier New', Courier, monospace;
    font-size: 0.85rem;
  }

  .terminal-line {
    margin-bottom: 0.4rem;
    line-height: 1.4;
  }

  .terminal-line .time {
    color: #64748b;
    margin-right: 0.5rem;
  }

  .terminal-line .tag {
    color: #38bdf8;
    margin-right: 0.5rem;
    font-weight: bold;
  }

  .terminal-line.ok .tag { color: #4ade80; }
  .terminal-line.error .tag { color: #f87171; }
  .terminal-line.warn .tag { color: #fbbf24; }

  .terminal-line.error .msg { color: #fca5a5; }
  .terminal-line.ok .msg { color: #86efac; }
  .terminal-line.info .msg { color: #cbd5e1; }

  .pulse {
    animation: pulse-anim 1.5s infinite;
  }

  @keyframes pulse-anim {
    0%, 100% { opacity: 0.5; }
    50% { opacity: 1; }
  }

  .stream-footer {
    display: flex;
    justify-content: flex-end;
    margin-top: 1rem;
  }

  .btn-close {
    background: transparent;
    border: none;
    color: #cbd5e1;
    font-size: 1.25rem;
    cursor: pointer;
  }
</style>
