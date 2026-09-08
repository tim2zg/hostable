<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost } from './api';
  import { toasts } from './toast';

  type CatalogItem = {
    name: string;
    title: string;
    category: string;
    description: string;
    logo: string;
    defaultMemory: string;
    variables: { key: string; label: string; default: string; type: string; help?: string }[];
    images: string[];
    volumes?: string[];
  };

  const catalogItems: CatalogItem[] = [
    {
      name: 'nextcloud-stack',
      title: 'Nextcloud (with Postgres & Redis)',
      category: 'Productivity',
      description: 'A fully self-contained Nextcloud instance backed by PostgreSQL and Redis for optimal performance. Natively merged without Docker!',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/6/60/Nextcloud_Logo.svg',
      defaultMemory: '2048',
      variables: [
        { key: 'DB_PASSWORD', label: 'Database Password', default: 'securepassword', type: 'password', help: 'Password for the PostgreSQL database' },
        { key: 'ADMIN_USER', label: 'Nextcloud Admin User', default: 'admin', type: 'text' },
        { key: 'ADMIN_PASSWORD', label: 'Nextcloud Admin Password', default: 'changeme', type: 'password' },
      ],
      images: [
        "postgres:15-alpine",
        "redis:alpine",
        "nextcloud:fpm-alpine"
      ],
      volumes: [
        "storage:8G:/var/lib/postgresql/data",
        "storage:1G:/data",
        "storage:10G:/var/www/html"
      ]
    },
    {
      name: 'wordpress-stack',
      title: 'WordPress (with MariaDB)',
      category: 'CMS',
      description: 'The world\'s most popular website builder, powered by MariaDB. Natively merged without Docker!',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/9/93/Wordpress_Blue_logo.png',
      defaultMemory: '1024',
      variables: [
        { key: 'DB_PASSWORD', label: 'Database Password', default: 'securepassword', type: 'password' },
      ],
      images: [
        "mariadb:10.6",
        "wordpress:fpm-alpine"
      ],
      volumes: [
        "storage:5G:/var/lib/mysql",
        "storage:5G:/var/www/html"
      ]
    },
    {
      name: 'plex-stack',
      title: 'Plex Media Server',
      category: 'Media',
      description: 'Organizes video, music and photos from personal media libraries and streams them to smart TVs.',
      logo: 'https://upload.wikimedia.org/wikipedia/commons/7/7b/Plex_logo_2022.svg',
      defaultMemory: '2048',
      variables: [
        { key: 'CLAIM_TOKEN', label: 'Plex Claim Token', default: '', type: 'text', help: 'Get one at plex.tv/claim' },
        { key: 'TZ', label: 'Timezone', default: 'Europe/London', type: 'text' },
      ],
      images: [
        "lscr.io/linuxserver/plex:latest"
      ],
      volumes: [
        "storage:10G:/config",
        "storage:50G:/media"
      ]
    }
  ];

  let showModal = false;
  let selectedApp: CatalogItem | null = null;
  let targetVmid = '';
  let isDeploying = false;
  let deployStatus = '';
  
  let userVariables: Record<string, string> = {};
  
  // Storage settings
  let memory = '1024';
  let templateStorage = 'local';
  let rootfsStorage = 'local-lvm';
  let availableStorages: any[] = [];
  let loading = true;

  onMount(async () => {
    try {
      const storageRes = await apiGet('/node/storages');
      if (storageRes && storageRes.data) {
        availableStorages = storageRes.data.filter((s: any) => s.content && s.content.includes('rootdir'));
      }
    } catch (e: any) {
      toasts.add(`Failed to load storages: ${e.message}`, 'error');
    } finally {
      loading = false;
    }
  });

  function openDeployModal(app: CatalogItem) {
    selectedApp = app;
    showModal = true;
    targetVmid = Math.floor(Math.random() * (900 - 200 + 1) + 200).toString();
    deployStatus = '';
    isDeploying = false;
    memory = app.defaultMemory;
    
    userVariables = {};
    app.variables.forEach(v => {
      userVariables[v.key] = v.default;
    });

    templateStorage = localStorage.getItem('hostable_tpl_storage') || 'local';
    rootfsStorage = localStorage.getItem('hostable_rootfs_storage') || 'local-lvm';
  }

  function closeDeployModal() {
    if (!isDeploying) {
      showModal = false;
      selectedApp = null;
    }
  }

  async function deployStack() {
    if (!selectedApp || !targetVmid) return;
    
    isDeploying = true;
    deployStatus = 'Preparing Stack Deployment...';

    try {
      // Interpolate rootfsStorage into volume definitions
      const actualVolumes = selectedApp.volumes 
        ? selectedApp.volumes.map(v => v.replace('storage', rootfsStorage))
        : [];

      const payload = {
        images: selectedApp.images,
        hostname: selectedApp.name,
        vmid: parseInt(targetVmid),
        memory,
        template_storage: templateStorage,
        rootfs_storage: rootfsStorage,
        env_vars: userVariables,
        volumes: actualVolumes
      };

      deployStatus = 'Deploying App Stack via LXC (this may take a few minutes)...';

      const res = await apiPost('/lxc/stack/deploy', payload);
      
      deployStatus = 'Success! Stack is running in the LXC.';
      toasts.add(`Successfully deployed ${selectedApp.name}!`, 'success');
      
      setTimeout(() => {
        closeDeployModal();
      }, 3000);
      
    } catch (err: any) {
      deployStatus = `Error: ${err.message}`;
      toasts.add(`Deployment failed: ${err.message}`, 'error', 10000);
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
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 1.5rem;
    margin-top: 1.5rem;
  }

  .app-card {
    background: var(--surface-color, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 12px;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    transition: transform 0.2s, box-shadow 0.2s;
  }

  .app-card:hover {
    transform: translateY(-4px);
    box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.3);
    border-color: #38bdf8;
  }

  .app-header {
    display: flex;
    align-items: center;
    gap: 1rem;
  }

  .app-logo {
    width: 60px;
    height: 60px;
    object-fit: contain;
    background: white;
    border-radius: 12px;
    padding: 0.5rem;
  }

  .app-info h3 {
    margin: 0 0 0.2rem 0;
    font-size: 1.2rem;
    color: #f8fafc;
  }

  .app-info .category {
    font-size: 0.85rem;
    color: #38bdf8;
    background: rgba(56, 189, 248, 0.1);
    padding: 0.2rem 0.6rem;
    border-radius: 20px;
    display: inline-block;
  }

  .app-desc {
    color: #94a3b8;
    font-size: 0.95rem;
    line-height: 1.5;
    flex-grow: 1;
  }

  /* Modal */
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(15, 23, 42, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 2000;
  }

  .modal-content {
    background: var(--surface-color, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 12px;
    width: 90%;
    max-width: 600px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    padding: 1.5rem;
    border-bottom: 1px solid #334155;
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  
  .modal-body {
    padding: 1.5rem;
    overflow-y: auto;
    flex-grow: 1;
  }
  
  .modal-footer {
    padding: 1.5rem;
    border-top: 1px solid #334155;
    display: flex;
    justify-content: flex-end;
    gap: 1rem;
  }

  .form-group {
    margin-bottom: 1.2rem;
  }
  .form-group label {
    display: block;
    color: #cbd5e1;
    margin-bottom: 0.5rem;
    font-weight: 500;
  }
  .form-group .help-text {
    font-size: 0.8rem;
    color: #64748b;
    margin-top: 0.2rem;
  }

  .input-field {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    color: #f8fafc;
    padding: 0.75rem;
    border-radius: 6px;
    font-size: 1rem;
  }
</style>

<div class="catalog-container animate-fade-in">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2.2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">App Store 2.0 (Stacks)</h1>
    <p style="color: #94a3b8; font-size: 1.05rem;">Deploy fully self-contained Docker Compose stacks inside optimized LXC containers.</p>
  </div>

  <div class="grid">
    {#each catalogItems as app}
      <div class="app-card">
        <div class="app-header">
          <img src={app.logo} alt={app.title} class="app-logo" />
          <div class="app-info">
            <h3>{app.title}</h3>
            <span class="category">{app.category}</span>
          </div>
        </div>
        <p class="app-desc">{app.description}</p>
        <button class="flat-btn-primary" style="width: 100%; background: #0ea5e9; color: white;" on:click={() => openDeployModal(app)}>
          🚀 Deploy Stack
        </button>
      </div>
    {/each}
  </div>

  {#if showModal && selectedApp}
    <div class="modal-backdrop" on:click={closeDeployModal} on:keydown={(e) => e.key === 'Escape' && closeDeployModal()} tabindex="0" role="button">
      <div class="modal-content" on:click|stopPropagation on:keydown|stopPropagation tabindex="0" role="dialog">
        <div class="modal-header">
          <div style="display: flex; align-items: center; gap: 1rem;">
            <img src={selectedApp.logo} alt={selectedApp.title} style="width: 40px; height: 40px; background: white; border-radius: 8px; padding: 4px;" />
            <h2 style="margin: 0; color: white; font-size: 1.3rem;">Deploy {selectedApp.title} Stack</h2>
          </div>
          <button class="flat-btn-outline" style="padding: 0.2rem 0.6rem; border: none;" on:click={closeDeployModal}>✕</button>
        </div>

        <div class="modal-body">
          <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; margin-bottom: 1.5rem;">
            <div class="form-group">
              <label for="vmid">LXC VMID</label>
              <input type="number" id="vmid" class="input-field" bind:value={targetVmid} disabled={isDeploying} />
            </div>
            <div class="form-group">
              <label for="memory">Memory (MB)</label>
              <input type="number" id="memory" class="input-field" bind:value={memory} disabled={isDeploying} />
            </div>
          </div>

          <div style="background: rgba(15, 23, 42, 0.5); padding: 1.5rem; border-radius: 8px; border: 1px solid #1e293b; margin-bottom: 1.5rem;">
            <h3 style="margin-top: 0; color: #e2e8f0; font-size: 1.1rem; margin-bottom: 1rem;">App Configuration</h3>
            
            {#each selectedApp.variables as variable}
              <div class="form-group">
                <label for={variable.key}>{variable.label}</label>
                <input 
                  type={variable.type} 
                  id={variable.key} 
                  class="input-field" 
                  bind:value={userVariables[variable.key]} 
                  disabled={isDeploying} 
                />
                {#if variable.help}
                  <div class="help-text">{variable.help}</div>
                {/if}
              </div>
            {/each}
          </div>

          <details style="background: rgba(15, 23, 42, 0.3); padding: 1rem; border-radius: 8px; border: 1px solid #1e293b;">
            <summary style="cursor: pointer; color: #cbd5e1; font-weight: 500;">Advanced Storage Settings</summary>
            <div style="margin-top: 1rem; display: grid; grid-template-columns: 1fr 1fr; gap: 1rem;">
              <div class="form-group">
                <label for="tpl">Template Storage</label>
                <select id="tpl" class="input-field" bind:value={templateStorage} disabled={isDeploying}>
                  {#each availableStorages as s}
                    <option value={s.storage}>{s.storage}</option>
                  {/each}
                </select>
              </div>
              <div class="form-group">
                <label for="rtfs">Rootfs Storage</label>
                <select id="rtfs" class="input-field" bind:value={rootfsStorage} disabled={isDeploying}>
                  {#each availableStorages as s}
                    <option value={s.storage}>{s.storage}</option>
                  {/each}
                </select>
              </div>
            </div>
          </details>

          {#if deployStatus}
            <div style="margin-top: 1.5rem; padding: 1rem; background: rgba(56, 189, 248, 0.1); border: 1px solid rgba(56, 189, 248, 0.2); border-radius: 8px; color: #38bdf8;">
              {deployStatus}
            </div>
          {/if}
        </div>

        <div class="modal-footer">
          <button class="flat-btn-outline" on:click={closeDeployModal} disabled={isDeploying}>Cancel</button>
          <button class="flat-btn-primary" style="background: #10b981; color: white;" on:click={deployStack} disabled={isDeploying || !targetVmid}>
            {isDeploying ? 'Deploying...' : '🚀 Launch Stack'}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>
