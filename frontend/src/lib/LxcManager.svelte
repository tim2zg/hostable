<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet, apiPost } from './api';
  import { toasts } from './toast';
  import { Terminal } from 'xterm';
  import { FitAddon } from 'xterm-addon-fit';
  import 'xterm/css/xterm.css';

  type Lxc = { id: number, name: string, status: string, mem: string, cpu: string, tags: string, type: string };
  let allContainers: Lxc[] = [];
  let containers: Lxc[] = [];
  let isFetching = false;
  let showHostableOnly = true;
  let searchQuery = '';

  // Terminal Console State
  let activeConsoleId: number | null = null;
  let ws: WebSocket | null = null;
  let terminalRef: HTMLElement;
  let xterm: Terminal | null = null;
  let fitAddon: FitAddon | null = null;

  async function fetchContainers() {
    isFetching = true;
    try {
      allContainers = await apiGet('/lxcs');
      applyFilter();
    } catch (err) {
      console.error("Failed to fetch LXCs", err);
      allContainers = [];
      containers = [];
    } finally {
      isFetching = false;
    }
  }

  function applyFilter() {
    let filtered = allContainers;
    if (showHostableOnly) {
      filtered = filtered.filter(ct => ct.tags && ct.tags.includes('hostable'));
    }
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      filtered = filtered.filter(ct => ct.name.toLowerCase().includes(q) || ct.id.toString().includes(q));
    }
    containers = filtered;
  }

  function toggleFilter() {
    showHostableOnly = !showHostableOnly;
    applyFilter();
  }

  async function startContainer(id: number) {
    try {
      await apiPost(`/lxc/${id}/start`, {});
      toasts.add(`Started instance #${id}`, 'success');
      await fetchContainers();
    } catch(e: any) { 
      toasts.add(`Failed to start #${id}: ${e.message}`, 'error', 5000); 
    }
  }

  async function stopContainer(id: number) {
    if (!confirm(`Are you sure you want to stop container #${id}?`)) return;
    try {
      await apiPost(`/lxc/${id}/stop`, {});
      toasts.add(`Stopped instance #${id}`, 'success');
      await fetchContainers();
    } catch(e: any) { 
      toasts.add(`Failed to stop #${id}: ${e.message}`, 'error', 5000); 
    }
  }

  async function restartContainer(id: number) {
    try {
      await apiPost(`/lxc/${id}/restart`, {});
      toasts.add(`Restarted instance #${id}`, 'success');
      await fetchContainers();
    } catch(e: any) { 
      toasts.add(`Failed to restart #${id}: ${e.message}`, 'error', 5000); 
    }
  }

  function openConsole(id: number) {
    activeConsoleId = id;
    
    // Give Svelte time to render the modal before mounting xterm
    setTimeout(() => {
      if (terminalRef) {
        xterm = new Terminal({
          cursorBlink: true,
          theme: {
            background: '#020617',
            foreground: '#e2e8f0',
            cursor: '#38bdf8'
          },
          fontFamily: 'monospace',
          fontSize: 14
        });
        
        fitAddon = new FitAddon();
        xterm.loadAddon(fitAddon);
        xterm.open(terminalRef);
        fitAddon.fit();

        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        ws = new WebSocket(`${protocol}//${window.location.host}/api/ws/${id}?token=${localStorage.getItem('hostable_token')}`);
        
        ws.onmessage = (event) => {
          xterm?.write(event.data);
        };

        ws.onclose = () => {
          xterm?.write("\r\n\x1b[31m[Console disconnected]\x1b[0m\r\n");
        };

        // Send keystrokes directly to WebSocket
        xterm.onData(data => {
          if (ws && ws.readyState === WebSocket.OPEN) {
            ws.send(data);
          }
        });

        // Handle window resize
        window.addEventListener('resize', handleResize);
      }
    }, 100);
  }

  function handleResize() {
    if (fitAddon) {
      fitAddon.fit();
    }
  }

  function closeConsole() {
    if (ws) {
      ws.close();
      ws = null;
    }
    if (xterm) {
      xterm.dispose();
      xterm = null;
    }
    window.removeEventListener('resize', handleResize);
    activeConsoleId = null;
  }

  onMount(() => {
    fetchContainers();
  });

  onDestroy(() => {
    closeConsole();
  });
</script>

<style>
  .lxc-list {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .lxc-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 1rem;
  }

  .status-dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
  }
  
  .status-dot.running {
    background: #22c55e;
    box-shadow: 0 0 8px rgba(34, 197, 94, 0.5);
  }

  .status-dot.stopped {
    background: #ef4444;
  }

  .actions {
    display: flex;
    gap: 0.8rem;
  }

  .lxc-stats {
    display: flex;
    gap: 2rem;
    color: #94a3b8;
    font-size: 0.95rem;
    margin-top: 1.5rem;
    border-top: 1px solid #334155;
    padding-top: 1rem;
  }
  
  .stat span {
    font-weight: 500;
    color: #cbd5e1;
  }

  /* Flat Card */
  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 1.5rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
    transition: transform 0.2s, border-color 0.2s, box-shadow 0.2s;
  }
  
  .flat-card:hover {
    border-color: #475569;
    box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05);
  }

  /* Console Terminal Modal (Glass/Premium) */
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(15, 23, 42, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    backdrop-filter: blur(8px);
  }

  .terminal-window {
    width: 90%;
    max-width: 1000px;
    height: 75vh;
    background: #020617;
    border: 1px solid #334155;
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  }

  .terminal-header {
    background: #0f172a;
    padding: 1rem 1.5rem;
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-family: monospace;
    color: #94a3b8;
    border-bottom: 1px solid #334155;
  }

  .close-btn {
    background: transparent;
    border: none;
    color: #ef4444;
    cursor: pointer;
    font-size: 1.5rem;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    transition: background 0.2s;
  }

  .close-btn:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  .terminal-body {
    flex-grow: 1;
    padding: 1rem;
    background: #020617;
    overflow: hidden;
  }

  .filter-controls {
    display: flex;
    gap: 1rem;
    align-items: center;
    margin-bottom: 2rem;
    flex-wrap: wrap;
  }
  
  .search-input {
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 8px;
    color: #e2e8f0;
    padding: 0.8rem 1rem;
    font-size: 0.95rem;
    outline: none;
    min-width: 300px;
    transition: border-color 0.2s;
  }
  .search-input:focus {
    border-color: #38bdf8;
  }

  .flat-btn-outline {
    background: transparent;
    border: 1px solid #475569;
    color: #cbd5e1;
    border-radius: 8px;
    padding: 0.8rem 1.2rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s;
  }
  .flat-btn-outline:hover {
    border-color: #38bdf8;
    color: #38bdf8;
    background: rgba(56, 189, 248, 0.05);
  }

  .flat-btn-primary {
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 8px;
    padding: 0.8rem 1.2rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.2s;
  }
  .flat-btn-primary:hover:not(:disabled) {
    background: #0ea5e9;
  }
</style>

<div class="animate-fade-in">
  <div style="display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1.5rem;">
    <div>
      <h1 style="font-size: 2.2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Cluster Instances</h1>
      <p style="color: #94a3b8; font-size: 1.05rem;">One-click lifecycle management and remote terminal diagnostics.</p>
    </div>
    <div style="display: flex; gap: 1rem;">
      <button class="flat-btn-primary" on:click={fetchContainers} disabled={isFetching}>
        {isFetching ? 'Refreshing...' : '🔄 Refresh List'}
      </button>
    </div>
  </div>

  <div class="filter-controls">
    <input 
      type="text" 
      class="search-input" 
      placeholder="Search instances by name or ID..." 
      bind:value={searchQuery}
      on:input={applyFilter}
    />
    <button class="flat-btn-outline" on:click={toggleFilter}>
      {showHostableOnly ? 'Showing: Hostable Managed' : 'Showing: All Proxmox Instances'}
    </button>
  </div>

  {#if containers.length === 0 && !isFetching}
    <div class="flat-card" style="text-align: center; color: #94a3b8; padding: 4rem;">
      <div style="font-size: 3rem; margin-bottom: 1rem;">📦</div>
      No instances found matching the current filter.
    </div>
  {/if}

  <div class="lxc-list">
    {#each containers as ct}
      <div class="flat-card">
        <div class="lxc-header">
          <div style="display: flex; align-items: center; gap: 1.2rem;">
            <div class="status-dot {ct.status}"></div>
            <h3 style="font-size: 1.3rem; font-weight: 600; color: #f8fafc; display: flex; align-items: center; gap: 0.8rem; margin: 0;">
              {ct.name} 
              <span style="color: #64748b; font-size: 0.9rem; font-weight: normal;">#{ct.id}</span>
              {#if ct.type === 'qemu'}
                <span style="background: #334155; color: #cbd5e1; font-size: 0.75rem; padding: 3px 8px; border-radius: 6px; font-weight: 600;">VM</span>
              {:else}
                <span style="background: #0ea5e9; color: #0f172a; font-size: 0.75rem; padding: 3px 8px; border-radius: 6px; font-weight: bold;">LXC</span>
              {/if}
              {#if ct.tags && ct.tags.includes('hostable')}
                <span style="background: rgba(56, 189, 248, 0.1); color: #38bdf8; border: 1px solid rgba(56, 189, 248, 0.2); font-size: 0.75rem; padding: 2px 8px; border-radius: 6px;">managed by hostable</span>
              {/if}
            </h3>
          </div>
          
          <div class="actions">
            {#if ct.status === 'running'}
              <button class="flat-btn-outline" title="Stop Instance" on:click={() => stopContainer(ct.id)}>⏹ Stop</button>
              <button class="flat-btn-outline" title="Restart Instance" on:click={() => restartContainer(ct.id)}>🔄 Restart</button>
              <button class="flat-btn-primary" title="Open Web Terminal Console" on:click={() => openConsole(ct.id)}>💻 Console</button>
            {:else}
              <button class="flat-btn-primary" title="Start Instance" on:click={() => startContainer(ct.id)}>▶ Start</button>
            {/if}
          </div>
        </div>
        
        <div class="lxc-stats">
          <div class="stat"><span>RAM Limit:</span> {ct.mem}</div>
          <div class="stat"><span>CPU Shares:</span> {ct.cpu}</div>
          <div class="stat"><span>Node:</span> pve</div>
        </div>
      </div>
    {/each}
  </div>

  <!-- Interactive Terminal Console Modal -->
  {#if activeConsoleId}
    <div class="modal-backdrop" on:click={closeConsole}>
      <div class="terminal-window" on:click|stopPropagation>
        <div class="terminal-header">
          <div style="display: flex; align-items: center; gap: 0.5rem;">
            <div class="status-dot running"></div>
            <span>root@container-{activeConsoleId}:~#</span>
          </div>
          <button class="close-btn" on:click={closeConsole}>×</button>
        </div>
        <div class="terminal-body" bind:this={terminalRef}></div>
      </div>
    </div>
  {/if}
</div>
