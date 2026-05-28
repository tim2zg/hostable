<script lang="ts">
  import { onMount } from 'svelte';

  let image = '';
  let hostname = '';
  let targetVmid = '';
  let memory = '512';
  let envVars: {key: string, value: string}[] = [];
  let volumes: {host: string, container: string}[] = [];
  let useHostableDb = false;
  let dbName = '';
  let templateStorage = 'local';
  let rootfsStorage = 'local-lvm';
  
  let isDeploying = false;
  let deployStatus = '';
  let deployProgress = 0;

  onMount(() => {
    targetVmid = Math.floor(Math.random() * (900 - 200 + 1) + 200).toString();
    templateStorage = localStorage.getItem('hostable_tpl_storage') || 'local';
    rootfsStorage = localStorage.getItem('hostable_rootfs_storage') || 'local-lvm';
  });

  async function deployApp() {
    if (!targetVmid || !image || !hostname) return;
    
    isDeploying = true;
    deployStatus = 'Preparing Deployment...';
    deployProgress = 20;

    try {
      const payload = {
        image,
        hostname,
        vmid: parseInt(targetVmid),
        memory,
        template_storage: templateStorage,
        rootfs_storage: rootfsStorage,
        env_vars: envVars.filter(e => e.key).map(e => `${e.key}=${e.value}`),
        volumes: volumes.filter(v => v.host).map(v => `${v.host}:${v.container}`),
        use_hostable_db: useHostableDb,
        db_name: useHostableDb ? dbName : null,
      };

      deployStatus = 'Deploying LXC Container (this may take a minute)...';
      deployProgress = 60;

      const deployRes = await fetch('/api/deploy', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
      
      const responseData = await deployRes.json();
      if (!deployRes.ok) throw new Error(responseData.message || 'Deployment failed');
      
      deployStatus = 'Success! Container deployed and starting.';
      deployProgress = 100;
      
    } catch (err: any) {
      deployStatus = `Error: ${err.message}`;
      isDeploying = false;
    }
  }

  function addEnv() { envVars = [...envVars, {key: '', value: ''}]; }
  function removeEnv(idx: number) { envVars = envVars.filter((_, i) => i !== idx); }
  function addVol() { volumes = [...volumes, {host: '', container: ''}]; }
  function removeVol(idx: number) { volumes = volumes.filter((_, i) => i !== idx); }
</script>

<style>
  .custom-deploy-container {
    max-width: 800px;
  }

  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 2rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  }

  .form-group {
    margin-bottom: 1.5rem;
  }

  .form-group label {
    display: block;
    color: #cbd5e1;
    font-size: 0.95rem;
    margin-bottom: 0.5rem;
    font-weight: 500;
  }

  .form-group input {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    color: #f8fafc;
    border-radius: 6px;
    padding: 0.8rem;
    font-size: 1rem;
    box-sizing: border-box;
  }

  .form-group input:focus {
    outline: none;
    border-color: #38bdf8;
  }

  .btn-primary {
    width: 100%;
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 6px;
    padding: 1rem;
    font-weight: 600;
    cursor: pointer;
    font-size: 1.1rem;
    transition: background 0.2s;
  }

  .btn-primary:hover:not(:disabled) {
    background: #0ea5e9;
  }

  .btn-primary:disabled {
    background: #475569;
    color: #94a3b8;
    cursor: not-allowed;
  }

  .progress-bar {
    width: 100%;
    height: 8px;
    background: #0f172a;
    border-radius: 4px;
    margin-top: 1rem;
    overflow: hidden;
  }
  
  .progress-fill {
    height: 100%;
    background: #38bdf8;
    transition: width 0.3s ease;
  }
</style>

<div class="custom-deploy-container animate-fade-in">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Custom Deploy</h1>
    <p style="color: #94a3b8; font-size: 1rem;">Deploy any Docker image directly to a Proxmox LXC container.</p>
  </div>

  <div class="flat-card">
    <div style="display: flex; gap: 1rem;">
      <div class="form-group" style="flex: 2;">
        <label for="image">Docker Image</label>
        <input id="image" type="text" bind:value={image} placeholder="e.g. redis:latest" disabled={isDeploying} />
      </div>
      <div class="form-group" style="flex: 1;">
        <label for="hostname">Hostname</label>
        <input id="hostname" type="text" bind:value={hostname} placeholder="e.g. redis-db" disabled={isDeploying} />
      </div>
    </div>

    <div style="display: flex; gap: 1rem;">
      <div class="form-group" style="flex: 1;">
        <label for="vmid">Target VMID</label>
        <input id="vmid" type="number" bind:value={targetVmid} disabled={isDeploying} />
      </div>
      <div class="form-group" style="flex: 1;">
        <label for="memory">Memory (MB)</label>
        <input id="memory" type="number" bind:value={memory} disabled={isDeploying} />
      </div>
    </div>

    <!-- Env Vars -->
    <div style="margin-bottom: 1.5rem;">
      <label style="display: flex; justify-content: space-between; color: #cbd5e1; font-size: 0.95rem; margin-bottom: 0.5rem; font-weight: 500;">
        Environment Variables
        <button style="background: none; border: none; color: #38bdf8; cursor: pointer; font-size: 0.9rem;" on:click={addEnv} disabled={isDeploying}>+ Add</button>
      </label>
      {#each envVars as env, i}
        <div style="display: flex; gap: 0.5rem; margin-bottom: 0.5rem;">
          <input type="text" placeholder="KEY" style="flex: 1; padding: 0.6rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 4px;" bind:value={env.key} disabled={isDeploying} />
          <input type="text" placeholder="VALUE" style="flex: 1; padding: 0.6rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 4px;" bind:value={env.value} disabled={isDeploying} />
          <button style="background: transparent; color: #ef4444; border: none; cursor: pointer; font-size: 1.2rem;" on:click={() => removeEnv(i)} disabled={isDeploying}>✖</button>
        </div>
      {/each}
    </div>

    <!-- Volumes -->
    <div style="margin-bottom: 1.5rem;">
      <label style="display: flex; justify-content: space-between; color: #cbd5e1; font-size: 0.95rem; margin-bottom: 0.5rem; font-weight: 500;">
        Volume Mappings
        <button style="background: none; border: none; color: #38bdf8; cursor: pointer; font-size: 0.9rem;" on:click={addVol} disabled={isDeploying}>+ Add</button>
      </label>
      {#each volumes as vol, i}
        <div style="display: flex; gap: 0.5rem; margin-bottom: 0.5rem;">
          <input type="text" placeholder="/host/path" style="flex: 1; padding: 0.6rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 4px;" bind:value={vol.host} disabled={isDeploying} />
          <input type="text" placeholder="/container/path" style="flex: 1; padding: 0.6rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 4px;" bind:value={vol.container} disabled={isDeploying} />
          <button style="background: transparent; color: #ef4444; border: none; cursor: pointer; font-size: 1.2rem;" on:click={() => removeVol(i)} disabled={isDeploying}>✖</button>
        </div>
      {/each}
    </div>

    <div style="margin-bottom: 2rem;">
      <label style="display: flex; align-items: center; gap: 0.5rem; color: #cbd5e1; font-size: 0.95rem; cursor: pointer; font-weight: 500;">
        <input type="checkbox" bind:checked={useHostableDb} disabled={isDeploying} />
        Provision Hostable PostgreSQL Database
      </label>
      {#if useHostableDb}
        <div style="margin-top: 0.5rem;">
          <input type="text" placeholder="Database Name" style="width: 100%; padding: 0.8rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 6px; box-sizing: border-box;" bind:value={dbName} disabled={isDeploying} />
        </div>
      {/if}
    </div>

    {#if deployStatus}
      <div style="margin-bottom: 1.5rem; color: #cbd5e1; font-size: 1rem; text-align: center;">
        {deployStatus}
        {#if isDeploying || deployProgress === 100}
          <div class="progress-bar">
            <div class="progress-fill" style="width: {deployProgress}%"></div>
          </div>
        {/if}
      </div>
    {/if}

    <button class="btn-primary" on:click={deployApp} disabled={isDeploying || !image || !hostname || !targetVmid}>
      {isDeploying ? 'Deploying...' : 'Deploy Image'}
    </button>
  </div>
</div>
