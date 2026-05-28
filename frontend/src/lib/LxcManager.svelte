<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost } from './api';

  type Lxc = { id: number, name: string, status: string, mem: string, cpu: string, tags: string, type: string };
  let allContainers: Lxc[] = [];
  let containers: Lxc[] = [];
  let isFetching = false;
  let showHostableOnly = true;
  let searchQuery = '';

  // Terminal Console State
  let activeConsoleId: number | null = null;
  let consoleOutput = "";
  let consoleInput = "";
  let ws: WebSocket | null = null;
  let terminalRef: HTMLElement;

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
      await fetchContainers();
    } catch(e) { console.error(e); }
  }

  async function stopContainer(id: number) {
    if (!confirm(`Are you sure you want to stop container #${id}?`)) return;
    try {
      await apiPost(`/lxc/${id}/stop`, {});
      await fetchContainers();
    } catch(e) { console.error(e); }
  }

  async function restartContainer(id: number) {
    try {
      await apiPost(`/lxc/${id}/restart`, {});
      await fetchContainers();
    } catch(e) { console.error(e); }
  }

  function openConsole(id: number) {
    activeConsoleId = id;
    consoleOutput = "";
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${protocol}//${window.location.host}/api/ws/${id}?token=${localStorage.getItem('hostable_token')}`);
    
    ws.onmessage = (event) => {
      consoleOutput += event.data;
      scrollToBottom();
    };

    ws.onclose = () => {
      consoleOutput += "\r\n[Console disconnected]\r\n";
    };
  }

  function closeConsole() {
    if (ws) {
      ws.close();
      ws = null;
    }
    activeConsoleId = null;
  }

  function sendCommand(e: KeyboardEvent) {
    if (e.key === 'Enter' && ws) {
      ws.send(consoleInput);
      consoleInput = "";
      scrollToBottom();
    }
  }

  function scrollToBottom() {
    setTimeout(() => {
      if (terminalRef) terminalRef.scrollTop = terminalRef.scrollHeight;
    }, 50);
  }

  onMount(() => {
    fetchContainers();
  });
</script>

<style>
  .lxc-list {
    display: flex;
    flex-direction: column;
    gap: 1rem;
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
    gap: 0.5rem;
  }

  .lxc-stats {
    display: flex;
    gap: 2rem;
    color: #94a3b8;
    font-size: 0.95rem;
    margin-top: 1rem;
    border-top: 1px solid #334155;
    padding-top: 1rem;
  }
  
  .stat span {
    font-weight: 600;
    color: #cbd5e1;
  }

  /* Console Terminal Modal (Flat Dark) */
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(15, 23, 42, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .terminal-window {
    width: 85%;
    max-width: 900px;
    height: 65vh;
    background: #020617;
    border: 1px solid #334155;
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  }

  .terminal-header {
    background: #0f172a;
    padding: 0.8rem 1.2rem;
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
    font-size: 1.2rem;
  }

  .terminal-body {
    flex-grow: 1;
    padding: 1.2rem;
    overflow-y: auto;
    font-family: monospace;
    color: #38bdf8;
    font-size: 0.95rem;
    line-height: 1.4;
    text-align: left;
  }

  .terminal-body pre {
    white-space: pre-wrap;
    margin: 0;
  }

  .terminal-input-line {
    display: flex;
    margin-top: 0.5rem;
    align-items: center;
    gap: 0.5rem;
  }

  .terminal-input-line input {
    flex-grow: 1;
    background: transparent;
    border: none;
    color: #4ade80;
    font-family: monospace;
    font-size: 0.95rem;
    outline: none;
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
    border-radius: 6px;
    color: #e2e8f0;
    padding: 0.5rem 0.8rem;
    font-size: 0.95rem;
    outline: none;
    min-width: 250px;
  }
  .search-input:focus {
    border-color: #38bdf8;
  }
</style>

<div class="animate-fade-in">
  <div style="display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1rem;">
    <div>
      <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Cluster Instances</h1>
      <p style="color: #94a3b8; font-size: 1rem;">One-click lifecycle management and remote terminal diagnostics.</p>
    </div>
    <div style="display: flex; gap: 1rem;">
      <button class="flat-btn-primary" on:click={fetchContainers} disabled={isFetching}>
        {isFetching ? 'Refreshing...' : '🔄 Refresh'}
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
      {showHostableOnly ? 'Showing: Hostable Only' : 'Showing: All Instances'}
    </button>
  </div>

  {#if containers.length === 0 && !isFetching}
    <div class="flat-card" style="text-align: center; color: #94a3b8; padding: 3rem;">
      No instances found matching the current filter.
    </div>
  {/if}

  <div class="lxc-list">
    {#each containers as ct}
      <div class="flat-card">
        <div class="lxc-header">
          <div style="display: flex; align-items: center; gap: 1rem;">
            <div class="status-dot {ct.status}"></div>
            <h3 style="font-size: 1.2rem; font-weight: 600; color: #f8fafc; display: flex; align-items: center; gap: 0.5rem;">
              {ct.name} 
              <span style="color: #64748b; font-size: 0.9rem; font-weight: normal;">#{ct.id}</span>
              {#if ct.type === 'qemu'}
                <span style="background: #334155; color: #cbd5e1; font-size: 0.75rem; padding: 2px 6px; border-radius: 4px;">VM</span>
              {:else}
                <span style="background: #0ea5e9; color: #0f172a; font-size: 0.75rem; padding: 2px 6px; border-radius: 4px; font-weight: bold;">LXC</span>
              {/if}
              {#if ct.tags && ct.tags.includes('hostable')}
                <span style="background: rgba(56, 189, 248, 0.1); color: #38bdf8; border: 1px solid rgba(56, 189, 248, 0.2); font-size: 0.75rem; padding: 1px 6px; border-radius: 4px;">hostable</span>
              {/if}
            </h3>
          </div>
          
          <div class="actions">
            {#if ct.status === 'running'}
              <button class="flat-btn-outline" title="Stop" on:click={() => stopContainer(ct.id)}>⏹ Stop</button>
              <button class="flat-btn-outline" title="Restart" on:click={() => restartContainer(ct.id)}>🔄 Restart</button>
              <button class="flat-btn-primary" title="Console" on:click={() => openConsole(ct.id)}>💻 Console</button>
            {:else}
              <button class="flat-btn-primary" title="Start" on:click={() => startContainer(ct.id)}>▶ Start</button>
            {/if}
          </div>
        </div>
        
        <div class="lxc-stats">
          <div class="stat"><span>RAM Limit:</span> {ct.mem}</div>
          <div class="stat"><span>CPU Shares:</span> {ct.cpu}</div>
          <div class="stat"><span>Monitoring:</span> Active</div>
        </div>
      </div>
    {/each}
  </div>

  <!-- Interactive Terminal Console Modal -->
  {#if activeConsoleId}
    <div class="modal-backdrop" on:click={closeConsole}>
      <div class="terminal-window" on:click|stopPropagation>
        <div class="terminal-header">
          <span>root@container-{activeConsoleId}:~#</span>
          <button class="close-btn" on:click={closeConsole}>✖</button>
        </div>
        <div class="terminal-body" bind:this={terminalRef}>
          <pre>{consoleOutput}</pre>
          <div class="terminal-input-line">
            <span style="color: #4ade80;">$</span>
            <input 
              type="text" 
              bind:value={consoleInput} 
              on:keydown={sendCommand} 
              autofocus 
              placeholder="Type command and press Enter..." 
            />
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>
