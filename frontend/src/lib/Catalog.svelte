<script lang="ts">
  import { onMount } from 'svelte';

  let images: any[] = [];
  let loading = true;
  let errorMsg = '';
  
  let showModal = false;
  let selectedApp: any = null;
  let targetVmid = '';
  let targetNode = 'pve';
  
  let isDeploying = false;
  let deployStatus = '';
  let deployProgress = 0;

  onMount(async () => {
    try {
      const res = await fetch('/api/catalog');
      if (!res.ok) throw new Error('Failed to fetch catalog');
      const json = await res.json();
      
      if (json.data && json.data.repositories && json.data.repositories.linuxserver) {
        images = json.data.repositories.linuxserver.filter((img: any) => !img.deprecated);
      } else {
        errorMsg = 'Invalid catalog format';
      }
    } catch (e: any) {
      errorMsg = e.message;
    } finally {
      loading = false;
    }
  });

  function openDeployModal(app: any) {
    selectedApp = app;
    showModal = true;
    targetVmid = Math.floor(Math.random() * (900 - 200 + 1) + 200).toString(); // Random VMID
    deployStatus = '';
    isDeploying = false;
    deployProgress = 0;
  }

  function closeDeployModal() {
    if (!isDeploying) {
      showModal = false;
      selectedApp = null;
    }
  }

  async function deployApp() {
    if (!targetVmid) return;
    
    isDeploying = true;
    deployStatus = 'Generating Template...';
    deployProgress = 20;

    try {
      // 1. Convert Dockerfile to Distrobuilder YAML
      const dockerfile = `FROM lscr.io/linuxserver/${selectedApp.name}:latest`;
      const convertRes = await fetch('/api/convert', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ dockerfile })
      });
      
      if (!convertRes.ok) throw new Error('Conversion failed');
      const convertData = await convertRes.json();
      const yaml = convertData.yaml;
      
      deployStatus = 'Deploying LXC Container...';
      deployProgress = 60;

      // 2. Deploy LXC Container
      // Use the same endpoint (in simulation it takes dockerfile payload, but we'll send it)
      const deployRes = await fetch('/api/deploy', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ dockerfile })
      });
      
      if (!deployRes.ok) throw new Error('Deployment failed');
      
      deployStatus = 'Success! Container deployed.';
      deployProgress = 100;
      
      setTimeout(() => {
        closeDeployModal();
      }, 2000);
      
    } catch (err: any) {
      deployStatus = `Error: ${err.message}`;
      isDeploying = false;
    }
  }
</script>

<style>
  .catalog-container {
    height: 100%;
    overflow-y: auto;
    padding-right: 1rem;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 1.5rem;
    margin-top: 1.5rem;
  }

  .card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    transition: transform 0.2s, border-color 0.2s;
  }

  .card:hover {
    transform: translateY(-2px);
    border-color: #475569;
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 1rem;
  }

  .card-logo {
    width: 48px;
    height: 48px;
    border-radius: 8px;
    background: #0f172a;
    object-fit: contain;
    padding: 4px;
    border: 1px solid #334155;
  }

  .card-title {
    font-size: 1.1rem;
    font-weight: 600;
    color: #f8fafc;
    margin: 0;
  }

  .card-category {
    font-size: 0.8rem;
    color: #94a3b8;
    background: #0f172a;
    padding: 2px 8px;
    border-radius: 12px;
    border: 1px solid #334155;
    display: inline-block;
    margin-top: 4px;
  }

  .card-desc {
    color: #cbd5e1;
    font-size: 0.9rem;
    line-height: 1.4;
    flex-grow: 1;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .deploy-btn {
    width: 100%;
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 6px;
    padding: 0.6rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.2s;
  }

  .deploy-btn:hover {
    background: #0ea5e9;
  }

  /* Modal Styles */
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(15, 23, 42, 0.8);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    backdrop-filter: blur(4px);
  }

  .modal {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 2rem;
    width: 400px;
    max-width: 90%;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  }

  .modal h2 {
    margin-top: 0;
    color: #f8fafc;
    font-size: 1.25rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid #334155;
    padding-bottom: 0.75rem;
  }

  .form-group {
    margin-bottom: 1rem;
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
    color: #f8fafc;
    border-radius: 6px;
    padding: 0.6rem;
    font-size: 0.95rem;
    box-sizing: border-box;
  }

  .form-group input:focus {
    outline: none;
    border-color: #38bdf8;
  }

  .modal-actions {
    display: flex;
    gap: 1rem;
    margin-top: 2rem;
  }

  .btn-cancel {
    flex: 1;
    background: transparent;
    border: 1px solid #475569;
    color: #cbd5e1;
    border-radius: 6px;
    padding: 0.6rem;
    cursor: pointer;
  }

  .btn-cancel:hover {
    background: #334155;
  }

  .btn-confirm {
    flex: 1;
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 6px;
    padding: 0.6rem;
    font-weight: 600;
    cursor: pointer;
  }

  .btn-confirm:hover:not(:disabled) {
    background: #0ea5e9;
  }

  .btn-confirm:disabled {
    background: #475569;
    color: #94a3b8;
    cursor: not-allowed;
  }
  
  .progress-bar {
    width: 100%;
    height: 6px;
    background: #0f172a;
    border-radius: 3px;
    margin-top: 1rem;
    overflow: hidden;
  }
  
  .progress-fill {
    height: 100%;
    background: #38bdf8;
    transition: width 0.3s ease;
  }
</style>

<div class="catalog-container animate-fade-in">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">App Catalog</h1>
    <p style="color: #94a3b8; font-size: 1rem;">1-Click deploy official LinuxServer.io templates to your Proxmox node.</p>
  </div>

  {#if errorMsg}
    <div style="padding: 1rem; background: rgba(239, 68, 68, 0.15); color: #f87171; border: 1px solid #ef4444; border-radius: 6px;">
      {errorMsg}
    </div>
  {/if}

  {#if loading}
    <div style="color: #94a3b8; text-align: center; padding: 3rem;">
      Loading Catalog from LinuxServer API...
    </div>
  {:else}
    <div class="grid">
      {#each images as app}
        <div class="card">
          <div class="card-header">
            <img src={app.project_logo || 'https://raw.githubusercontent.com/linuxserver/docker-templates/master/linuxserver.io/img/linuxserver-ls-logo.png'} alt={app.name} class="card-logo" on:error={(e) => e.currentTarget.src='https://raw.githubusercontent.com/linuxserver/docker-templates/master/linuxserver.io/img/linuxserver-ls-logo.png'}/>
            <div>
              <h3 class="card-title">{app.name}</h3>
              {#if app.category}
                <span class="card-category">{app.category.split(',')[0]}</span>
              {/if}
            </div>
          </div>
          <div class="card-desc" title={app.description}>
            {app.description}
          </div>
          <div style="margin-top: auto;">
            <button class="deploy-btn" on:click={() => openDeployModal(app)}>1-Click Deploy</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

{#if showModal}
  <div class="modal-backdrop" on:click|self={closeDeployModal}>
    <div class="modal animate-fade-in">
      <h2>Deploy {selectedApp?.name}</h2>
      
      <div class="form-group">
        <label for="node">Target Node</label>
        <input id="node" type="text" bind:value={targetNode} disabled={isDeploying} />
      </div>
      
      <div class="form-group">
        <label for="vmid">LXC Container ID</label>
        <input id="vmid" type="number" bind:value={targetVmid} disabled={isDeploying} />
      </div>
      
      {#if deployStatus}
        <div style="margin-top: 1.5rem; color: #cbd5e1; font-size: 0.9rem; text-align: center;">
          {deployStatus}
          {#if isDeploying || deployProgress === 100}
            <div class="progress-bar">
              <div class="progress-fill" style="width: {deployProgress}%"></div>
            </div>
          {/if}
        </div>
      {/if}

      <div class="modal-actions">
        <button class="btn-cancel" on:click={closeDeployModal} disabled={isDeploying}>Cancel</button>
        <button class="btn-confirm" on:click={deployApp} disabled={isDeploying || !targetVmid}>
          {isDeploying ? 'Deploying...' : 'Deploy Now'}
        </button>
      </div>
    </div>
  </div>
{/if}
