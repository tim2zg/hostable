<script lang="ts">
  import { onMount } from 'svelte';

  type Lxc = { id: number, name: string, status: string, mem: string, cpu: string };
  let containers: Lxc[] = [];
  let isFetching = false;

  // Terminal Console State
  let activeConsoleId: number | null = null;
  let consoleOutput = "";
  let consoleInput = "";
  let ws: WebSocket | null = null;
  let terminalRef: HTMLElement;

  async function fetchContainers() {
    isFetching = true;
    try {
      const res = await fetch('/api/lxcs');
      if (res.ok) {
        containers = await res.json();
      }
    } catch (err) {
      console.error("Failed to fetch LXCs", err);
      // Mock fallback
      containers = [
        { id: 101, name: "nginx-proxy", status: "running", mem: "128MB / 1GB", cpu: "1%" },
        { id: 102, name: "postgres-db", status: "stopped", mem: "0MB / 2GB", cpu: "0%" },
        { id: 103, name: "pihole", status: "running", mem: "84MB / 512MB", cpu: "0.5%" },
      ];
    } finally {
      isFetching = false;
    }
  }

  function openConsole(id: number) {
    activeConsoleId = id;
    consoleOutput = "";
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    // Connect directly to backend Axum WebSocket
    ws = new WebSocket(`${protocol}//127.0.0.1:3000/api/ws/${id}`);
    
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

  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.5rem 2rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
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

  .flat-btn-outline {
    background: transparent;
    border: 1px solid #334155;
    color: #cbd5e1;
    border-radius: 6px;
    padding: 0.4rem 0.8rem;
    font-size: 0.9rem;
    cursor: pointer;
    transition: all 0.2s;
  }

  .flat-btn-outline:hover {
    border-color: #38bdf8;
    color: #38bdf8;
  }

  .flat-btn-primary {
    background: #38bdf8;
    border: none;
    color: #0f172a;
    border-radius: 6px;
    padding: 0.4rem 0.8rem;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.2s;
  }

  .flat-btn-primary:hover {
    background: #0ea5e9;
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
</style>

<div class="animate-fade-in">
  <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 2rem;">
    <div>
      <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">LXC Containers</h1>
      <p style="color: #94a3b8; font-size: 1rem;">One-click lifecycle management and remote terminal diagnostics.</p>
    </div>
    <button class="flat-btn-primary" on:click={fetchContainers} disabled={isFetching}>
      {isFetching ? 'Refreshing...' : '🔄 Refresh'}
    </button>
  </div>

  <div class="lxc-list">
    {#each containers as ct}
      <div class="flat-card">
        <div class="lxc-header">
          <div style="display: flex; align-items: center; gap: 1rem;">
            <div class="status-dot {ct.status}"></div>
            <h3 style="font-size: 1.2rem; font-weight: 600; color: #f8fafc;">
              {ct.name} 
              <span style="color: #64748b; font-size: 0.9rem; font-weight: normal;">#{ct.id}</span>
            </h3>
          </div>
          
          <div class="actions">
            {#if ct.status === 'running'}
              <button class="flat-btn-outline" title="Stop">⏹ Stop</button>
              <button class="flat-btn-outline" title="Restart">🔄 Restart</button>
              <button class="flat-btn-primary" title="Console" on:click={() => openConsole(ct.id)}>💻 Console</button>
            {:else}
              <button class="flat-btn-primary" title="Start">▶ Start</button>
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
