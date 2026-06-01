<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost } from './api';
  import { toasts } from './toast';

  const featuredApps = [
    {
      name: 'nextcloud',
      title: 'Nextcloud',
      category: 'Productivity',
      description: 'A safe home for all your data. Access & share your files, calendars, contacts, mail & more.',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/6/60/Nextcloud_Logo.svg',
      image: 'lscr.io/linuxserver/nextcloud:latest',
      defaultMemory: '1024',
      wizard: [
        { key: 'PUID', label: 'User ID (PUID)', value: '1000' },
        { key: 'PGID', label: 'Group ID (PGID)', value: '1000' },
        { key: 'TZ', label: 'Timezone', value: 'Europe/London' },
      ],
      volumes: [
        { host: '/mnt/storage/nextcloud/config', container: '/config', label: 'Configuration Data' },
        { host: '/mnt/storage/nextcloud/data', container: '/data', label: 'User Files Data' }
      ],
      recommendDb: true
    },
    {
      name: 'plex',
      title: 'Plex Media Server',
      category: 'Media',
      description: 'Organizes video, music and photos from personal media libraries and streams them to smart TVs.',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/7/7b/Plex_logo_2022.svg',
      image: 'lscr.io/linuxserver/plex:latest',
      defaultMemory: '2048',
      wizard: [
        { key: 'PUID', label: 'User ID (PUID)', value: '1000' },
        { key: 'PGID', label: 'Group ID (PGID)', value: '1000' },
        { key: 'TZ', label: 'Timezone', value: 'Europe/London' },
        { key: 'VERSION', label: 'Plex Pass Version', value: 'docker' }
      ],
      volumes: [
        { host: '/mnt/storage/plex/config', container: '/config', label: 'Plex Database' },
        { host: '/mnt/storage/media/movies', container: '/movies', label: 'Movies Directory' },
        { host: '/mnt/storage/media/tv', container: '/tv', label: 'TV Shows Directory' }
      ],
      recommendDb: false
    },
    {
      name: 'pihole',
      title: 'Pi-hole',
      category: 'Network',
      description: 'A black hole for Internet advertisements. Network-wide ad blocking.',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/0/0c/Pi-hole_Logo.png',
      image: 'pihole/pihole:latest',
      defaultMemory: '512',
      wizard: [
        { key: 'TZ', label: 'Timezone', value: 'Europe/London' },
        { key: 'WEBPASSWORD', label: 'Web Admin Password', value: 'admin' }
      ],
      volumes: [
        { host: '/mnt/storage/pihole/etc', container: '/etc/pihole', label: 'Pi-hole Config' },
        { host: '/mnt/storage/pihole/dnsmasq', container: '/etc/dnsmasq.d', label: 'DNSMasq Config' }
      ],
      recommendDb: false
    },
    {
      name: 'nginx-proxy-manager',
      title: 'Nginx Proxy Manager',
      category: 'Network',
      description: 'Docker container for managing Nginx proxy hosts with a simple, powerful interface.',
      logo: 'https://raw.githubusercontent.com/NginxProxyManager/nginx-proxy-manager/master/frontend/images/logo.png',
      image: 'jc21/nginx-proxy-manager:latest',
      defaultMemory: '1024',
      wizard: [],
      volumes: [
        { host: '/mnt/storage/npm/data', container: '/data', label: 'Proxy Configs' },
        { host: '/mnt/storage/npm/letsencrypt', container: '/etc/letsencrypt', label: 'SSL Certificates' }
      ],
      recommendDb: true
    }
  ];

  let images: any[] = [];
  let loading = true;
  let errorMsg = '';
  
  let currentTab: 'featured' | 'linuxserver' = 'featured';

  let showModal = false;
  let selectedApp: any = null;
  let isFeaturedApp = false;
  
  let targetVmid = '';
  let targetNode = 'pve';
  
  let isDeploying = false;
  let deployStatus = '';
  let deployProgress = 0;

  let memory = '512';
  let envVars: {key: string, value: string}[] = [];
  let volumes: {host: string, container: string}[] = [];
  let useHostableDb = false;
  let dbName = '';
  let templateStorage = 'local';
  let rootfsStorage = 'local-lvm';

  onMount(async () => {
    try {
      const json = await apiGet('/catalog');
      
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

  function openDeployModal(app: any, featured: boolean = false) {
    selectedApp = app;
    isFeaturedApp = featured;
    showModal = true;
    targetVmid = Math.floor(Math.random() * (900 - 200 + 1) + 200).toString(); // Random VMID
    deployStatus = '';
    isDeploying = false;
    deployProgress = 0;
    
    if (featured) {
      memory = app.defaultMemory || '512';
      envVars = app.wizard ? JSON.parse(JSON.stringify(app.wizard)) : [];
      volumes = app.volumes ? JSON.parse(JSON.stringify(app.volumes)) : [];
      useHostableDb = app.recommendDb || false;
    } else {
      memory = '512';
      envVars = [];
      volumes = [];
      useHostableDb = false;
    }
    
    dbName = (app.name || app.title).replace(/[^a-zA-Z0-9]/g, '');
    templateStorage = localStorage.getItem('hostable_tpl_storage') || 'local';
    rootfsStorage = localStorage.getItem('hostable_rootfs_storage') || 'local-lvm';
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
    deployStatus = 'Preparing Deployment...';
    deployProgress = 20;

    try {
      let imageString = isFeaturedApp ? selectedApp.image : `lscr.io/linuxserver/${selectedApp.name}:latest`;
      let hostname = isFeaturedApp ? selectedApp.name : selectedApp.name;

      const payload = {
        image: imageString,
        hostname: hostname,
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

      const responseData = await apiPost('/deploy', payload);
      
      deployStatus = 'Success! Container deployed and starting.';
      toasts.add(`Successfully deployed ${hostname}!`, 'success');
      deployProgress = 100;
      
      setTimeout(() => {
        closeDeployModal();
      }, 3000);
      
    } catch (err: any) {
      deployStatus = `Error: ${err.message}`;
      toasts.add(`Deployment failed: ${err.message}`, 'error', 5000);
      isDeploying = false;
    }
  }

  function addEnv() { envVars = [...envVars, {key: '', value: ''}]; }
  function removeEnv(idx: number) { envVars = envVars.filter((_, i) => i !== idx); }
  function addVol() { volumes = [...volumes, {host: '', container: ''}]; }
  function removeVol(idx: number) { volumes = volumes.filter((_, i) => i !== idx); }
</script>

<style>
  .catalog-container {
    height: 100%;
    overflow-y: auto;
    padding-right: 1rem;
  }

  .tabs {
    display: flex;
    gap: 1rem;
    margin-bottom: 2rem;
    border-bottom: 1px solid #334155;
    padding-bottom: 0.5rem;
  }

  .tab {
    background: transparent;
    border: none;
    color: #94a3b8;
    font-size: 1.1rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0.5rem 1rem;
    border-radius: 6px;
    transition: all 0.2s;
  }

  .tab.active {
    color: #38bdf8;
    background: rgba(56, 189, 248, 0.1);
  }

  .tab:hover:not(.active) {
    color: #cbd5e1;
    background: rgba(255, 255, 255, 0.05);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 1.5rem;
  }

  .card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    transition: transform 0.2s, border-color 0.2s, box-shadow 0.2s;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
  }

  .card:hover {
    transform: translateY(-4px);
    border-color: #475569;
    box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 1rem;
  }

  .card-logo {
    width: 54px;
    height: 54px;
    border-radius: 12px;
    background: #0f172a;
    object-fit: contain;
    padding: 6px;
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
    line-height: 1.5;
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
    border-radius: 8px;
    padding: 0.8rem;
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
    backdrop-filter: blur(8px);
  }

  .modal {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 2rem;
    width: 500px;
    max-width: 90%;
    max-height: 90vh;
    overflow-y: auto;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  }

  .modal h2 {
    margin-top: 0;
    color: #f8fafc;
    font-size: 1.5rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid #334155;
    padding-bottom: 1rem;
    display: flex;
    align-items: center;
    gap: 1rem;
  }

  .modal h2 img {
    width: 32px;
    height: 32px;
    border-radius: 6px;
  }

  .form-group {
    margin-bottom: 1.2rem;
  }

  .form-group label {
    display: block;
    color: #94a3b8;
    font-size: 0.9rem;
    margin-bottom: 0.4rem;
    font-weight: 500;
  }

  .form-group input {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    color: #f8fafc;
    border-radius: 8px;
    padding: 0.8rem;
    font-size: 0.95rem;
    box-sizing: border-box;
    transition: border-color 0.2s;
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
    border-radius: 8px;
    padding: 0.8rem;
    cursor: pointer;
    font-weight: 500;
  }

  .btn-cancel:hover {
    background: #334155;
  }

  .btn-confirm {
    flex: 1;
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 8px;
    padding: 0.8rem;
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

  .wizard-section {
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1rem;
    margin-bottom: 1.2rem;
  }

  .wizard-section h3 {
    margin: 0 0 1rem 0;
    color: #e2e8f0;
    font-size: 1rem;
    font-weight: 500;
  }
</style>

<div class="catalog-container animate-fade-in">
  <div style="margin-bottom: 1rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">App Catalog</h1>
    <p style="color: #94a3b8; font-size: 1rem;">1-Click deploy templates seamlessly to your Proxmox node.</p>
  </div>

  <div class="tabs">
    <button class="tab {currentTab === 'featured' ? 'active' : ''}" on:click={() => currentTab = 'featured'}>
      ⭐ Featured Apps
    </button>
    <button class="tab {currentTab === 'linuxserver' ? 'active' : ''}" on:click={() => currentTab = 'linuxserver'}>
      🐧 LinuxServer.io
    </button>
  </div>

  {#if errorMsg}
    <div style="padding: 1rem; background: rgba(239, 68, 68, 0.15); color: #f87171; border: 1px solid #ef4444; border-radius: 8px; margin-bottom: 1rem;">
      {errorMsg}
    </div>
  {/if}

  {#if currentTab === 'featured'}
    <div class="grid animate-fade-in">
      {#each featuredApps as app}
        <div class="card">
          <div class="card-header">
            <img src={app.logo} alt={app.title} class="card-logo"/>
            <div>
              <h3 class="card-title">{app.title}</h3>
              <span class="card-category">{app.category}</span>
            </div>
          </div>
          <div class="card-desc" title={app.description}>
            {app.description}
          </div>
          <div style="margin-top: auto;">
            <button class="deploy-btn" on:click={() => openDeployModal(app, true)}>1-Click Deploy</button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    {#if loading}
      <div style="color: #94a3b8; text-align: center; padding: 3rem;">
        Loading Catalog from LinuxServer API...
      </div>
    {:else}
      <div class="grid animate-fade-in">
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
              <button class="deploy-btn" on:click={() => openDeployModal(app, false)}>Deploy Instance</button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

{#if showModal}
  <div class="modal-backdrop" on:click|self={closeDeployModal}>
    <div class="modal animate-fade-in">
      <h2>
        {#if isFeaturedApp}
          <img src={selectedApp.logo} alt="logo" />
        {/if}
        Deploy {isFeaturedApp ? selectedApp.title : selectedApp?.name}
      </h2>
      
      <div style="display: flex; gap: 1rem;">
        <div class="form-group" style="flex: 1;">
          <label for="vmid">Target Container ID (VMID)</label>
          <input id="vmid" type="number" bind:value={targetVmid} disabled={isDeploying} />
        </div>
        <div class="form-group" style="flex: 1;">
          <label for="memory">Memory (MB)</label>
          <input id="memory" type="number" bind:value={memory} disabled={isDeploying} />
        </div>
      </div>

      {#if isFeaturedApp}
        <!-- SMART WIZARD MODE -->
        {#if envVars.length > 0}
          <div class="wizard-section">
            <h3>Configuration</h3>
            {#each envVars as env, i}
              <div class="form-group" style="margin-bottom: 0.8rem;">
                <label>{env.label || env.key}</label>
                <input type="text" bind:value={env.value} disabled={isDeploying} />
              </div>
            {/each}
          </div>
        {/if}

        {#if volumes.length > 0}
          <div class="wizard-section">
            <h3>Storage Mounts</h3>
            {#each volumes as vol, i}
              <div class="form-group" style="margin-bottom: 0.8rem;">
                <label>{vol.label || vol.container}</label>
                <input type="text" bind:value={vol.host} disabled={isDeploying} placeholder="/host/path" />
                <div style="font-size: 0.8rem; color: #64748b; margin-top: 0.2rem;">Mounts to: {vol.container} inside container</div>
              </div>
            {/each}
          </div>
        {/if}
      {:else}
        <!-- EXPERT MODE (RAW ENV / VOLUMES) -->
        <div class="wizard-section">
          <label style="display: flex; justify-content: space-between; color: #e2e8f0; font-size: 1rem; margin-bottom: 1rem; font-weight: 500;">
            Environment Variables
            <button style="background: none; border: none; color: #38bdf8; cursor: pointer; font-size: 0.85rem;" on:click={addEnv} disabled={isDeploying}>+ Add</button>
          </label>
          {#each envVars as env, i}
            <div style="display: flex; gap: 0.5rem; margin-bottom: 0.8rem;">
              <input type="text" placeholder="KEY" style="flex: 1; padding: 0.6rem; background: #1e293b; border: 1px solid #475569; color: #fff; border-radius: 6px;" bind:value={env.key} disabled={isDeploying} />
              <input type="text" placeholder="VALUE" style="flex: 1; padding: 0.6rem; background: #1e293b; border: 1px solid #475569; color: #fff; border-radius: 6px;" bind:value={env.value} disabled={isDeploying} />
              <button style="background: transparent; color: #ef4444; border: none; cursor: pointer; font-size: 1.2rem;" on:click={() => removeEnv(i)} disabled={isDeploying}>✖</button>
            </div>
          {/each}
        </div>

        <div class="wizard-section">
          <label style="display: flex; justify-content: space-between; color: #e2e8f0; font-size: 1rem; margin-bottom: 1rem; font-weight: 500;">
            Volume Mappings
            <button style="background: none; border: none; color: #38bdf8; cursor: pointer; font-size: 0.85rem;" on:click={addVol} disabled={isDeploying}>+ Add</button>
          </label>
          {#each volumes as vol, i}
            <div style="display: flex; gap: 0.5rem; margin-bottom: 0.8rem;">
              <input type="text" placeholder="/host/path" style="flex: 1; padding: 0.6rem; background: #1e293b; border: 1px solid #475569; color: #fff; border-radius: 6px;" bind:value={vol.host} disabled={isDeploying} />
              <input type="text" placeholder="/container/path" style="flex: 1; padding: 0.6rem; background: #1e293b; border: 1px solid #475569; color: #fff; border-radius: 6px;" bind:value={vol.container} disabled={isDeploying} />
              <button style="background: transparent; color: #ef4444; border: none; cursor: pointer; font-size: 1.2rem;" on:click={() => removeVol(i)} disabled={isDeploying}>✖</button>
            </div>
          {/each}
        </div>
      {/if}

      <div class="wizard-section" style="border-color: #38bdf8; background: rgba(56, 189, 248, 0.05);">
        <label style="display: flex; align-items: center; gap: 0.8rem; color: #e2e8f0; font-size: 0.95rem; cursor: pointer; font-weight: 500;">
          <input type="checkbox" bind:checked={useHostableDb} disabled={isDeploying} style="width: 1.2rem; height: 1.2rem; cursor: pointer;" />
          Provision Managed PostgreSQL Database
        </label>
        {#if useHostableDb}
          <div style="margin-top: 1rem;">
            <label style="color: #94a3b8; font-size: 0.85rem; margin-bottom: 0.4rem; display: block;">Database Name (Alphanumeric only)</label>
            <input type="text" placeholder="e.g. nextclouddb" style="width: 100%; padding: 0.8rem; background: #0f172a; border: 1px solid #334155; color: #fff; border-radius: 8px; box-sizing: border-box;" bind:value={dbName} disabled={isDeploying} />
          </div>
        {/if}
      </div>
      
      {#if deployStatus}
        <div style="margin-top: 1.5rem; color: #cbd5e1; font-size: 0.95rem; text-align: center; font-weight: 500;">
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
          {isDeploying ? 'Deploying...' : 'Deploy Instance Now'}
        </button>
      </div>
    </div>
  </div>
{/if}
