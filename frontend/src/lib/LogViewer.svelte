<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  type Lxc = { id: number, name: string, status: string };
  let containers: Lxc[] = [];
  let selectedVmid = "";
  
  let logs: string[] = [];
  let filterText = "";
  let isPaused = false;
  let autoScroll = true;
  let ws: WebSocket | null = null;
  let logBoxRef: HTMLElement;

  async function fetchContainers() {
    try {
      const res = await fetch('/api/lxcs');
      if (res.ok) {
        containers = await res.json();
        if (containers.length > 0) {
          selectedVmid = containers[0].id.toString();
          connectWebSocket();
        }
      }
    } catch (err) {
      console.error("Failed to load containers for log viewer", err);
    }
  }

  function connectWebSocket() {
    if (ws) {
      ws.close();
      ws = null;
    }
    if (!selectedVmid || isPaused) return;

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${protocol}//127.0.0.1:3000/api/ws/logs/${selectedVmid}`);

    ws.onmessage = (event) => {
      if (!isPaused) {
        logs = [...logs, event.data];
        if (logs.length > 1000) {
          logs = logs.slice(-1000); // Prevent memory leak by capping logs buffer
        }
        if (autoScroll) {
          scrollToBottom();
        }
      }
    };

    ws.onclose = () => {
      console.log(`WebSocket logs for VM ${selectedVmid} closed.`);
    };
  }

  function handleVmidChange() {
    logs = [];
    connectWebSocket();
  }

  function togglePause() {
    isPaused = !isPaused;
    if (isPaused) {
      if (ws) {
        ws.close();
        ws = null;
      }
    } else {
      connectWebSocket();
    }
  }

  function clearLogs() {
    logs = [];
  }

  function scrollToBottom() {
    setTimeout(() => {
      if (logBoxRef) {
        logBoxRef.scrollTop = logBoxRef.scrollHeight;
      }
    }, 50);
  }

  $: filteredLogs = logs.filter(line => 
    line.toLowerCase().includes(filterText.toLowerCase())
  );

  onMount(() => {
    fetchContainers();
  });

  onDestroy(() => {
    if (ws) ws.close();
  });
</script>

<style>
  .log-viewer-layout {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    height: calc(100vh - 12rem);
    background: #0f172a;
    color: #e2e8f0;
    font-family: 'Outfit', sans-serif;
  }

  .flat-header-bar {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1rem 1.5rem;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 1.2rem;
  }

  .control-group {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .flat-select, .flat-input {
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    color: #e2e8f0;
    padding: 0.5rem 0.8rem;
    font-size: 0.9rem;
    outline: none;
    font-family: inherit;
  }

  .flat-select:focus, .flat-input:focus {
    border-color: #38bdf8;
  }

  .flat-btn-outline {
    background: transparent;
    border: 1px solid #334155;
    color: #cbd5e1;
    border-radius: 6px;
    padding: 0.5rem 1rem;
    font-size: 0.9rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s;
  }

  .flat-btn-outline:hover {
    border-color: #38bdf8;
    color: #38bdf8;
    background: rgba(56, 189, 248, 0.05);
  }

  .flat-btn-outline.active {
    background: #38bdf8;
    color: #0f172a;
    border-color: #38bdf8;
  }

  .flat-log-box {
    flex-grow: 1;
    background: #020617;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.2rem;
    overflow-y: auto;
    font-family: monospace;
    font-size: 0.9rem;
    line-height: 1.5;
    color: #38bdf8;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .log-line {
    white-space: pre-wrap;
    word-break: break-all;
  }

  .no-logs {
    color: #475569;
    text-align: center;
    padding: 3rem 0;
    font-size: 0.95rem;
  }
</style>

<div class="log-viewer-layout animate-fade-in">
  <!-- Controls Header Panel -->
  <header class="flat-header-bar">
    <!-- Container Selector Dropdown -->
    <div class="control-group">
      <label for="lxc-select" style="font-size: 0.9rem; color: #94a3b8;">Container:</label>
      <select id="lxc-select" class="flat-select" bind:value={selectedVmid} on:change={handleVmidChange}>
        {#if containers.length === 0}
          <option value="">No containers found</option>
        {:else}
          {#each containers as container}
            <option value={container.id.toString()}>
              {container.name} (#{container.id})
            </option>
          {/each}
        {/if}
      </select>
    </div>

    <!-- Live Stream Control Pause/Resume -->
    <button class="flat-btn-outline {isPaused ? 'active' : ''}" on:click={togglePause}>
      {isPaused ? '▶ Resume Stream' : '⏸ Pause Stream'}
    </button>

    <!-- Clear logs output screen -->
    <button class="flat-btn-outline" on:click={clearLogs}>
      🗑 Clear Screen
    </button>

    <!-- Auto Scroll Toggle switch -->
    <div class="control-group">
      <input id="autoscroll-chk" type="checkbox" bind:checked={autoScroll} style="cursor: pointer; accent-color: #38bdf8;" />
      <label for="autoscroll-chk" style="font-size: 0.9rem; color: #cbd5e1; cursor: pointer;">Auto-scroll</label>
    </div>

    <!-- Live Search Filter Box -->
    <div class="control-group" style="margin-left: auto;">
      <input 
        type="text" 
        class="flat-input" 
        placeholder="Filter logs..." 
        bind:value={filterText} 
      />
    </div>
  </header>

  <!-- Log Box Output -->
  <div class="flat-log-box" bind:this={logBoxRef}>
    {#if filteredLogs.length === 0}
      <div class="no-logs">
        {isPaused ? 'Logs streaming paused.' : 'Waiting for connection to stream container logs...'}
      </div>
    {:else}
      {#each filteredLogs as line}
        <div class="log-line">{line}</div>
      {/each}
    {/if}
  </div>
</div>
