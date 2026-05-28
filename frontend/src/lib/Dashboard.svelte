<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet } from './api';

  let stats = {
    cpu: 0,
    ram: 0,
    disk: 0,
    activeLxcs: 0
  };
  let errorMsg = "";
  let interval: any;

  async function fetchStats() {
    if (document.hidden) return;
    try {
      stats = await apiGet('/stats');
      errorMsg = "";
    } catch (err) {
      console.error(err);
      errorMsg = "Unable to connect to Proxmox API or Backend.";
    }
  }

  onMount(() => {
    fetchStats();
    interval = setInterval(fetchStats, 5000);
    document.addEventListener('visibilitychange', fetchStats);
  });

  onDestroy(() => {
    clearInterval(interval);
    document.removeEventListener('visibilitychange', fetchStats);
  });
</script>

<style>
  .dashboard-container {
    display: flex;
    flex-direction: column;
    gap: 2rem;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 1.5rem;
  }

  .flat-card h3 {
    font-size: 1.1rem;
    color: var(--text-secondary);
    font-weight: 500;
    margin-bottom: 1rem;
  }

  .stat-value {
    font-size: 2.5rem;
    font-weight: 700;
    color: var(--primary-color);
    margin-bottom: 1rem;
  }

  .subtitle {
    color: var(--success-color);
    font-size: 0.9rem;
    font-weight: 500;
  }

  .alert-banner {
    padding: 1rem;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--danger-color);
    color: #f87171;
    border-radius: 6px;
    font-size: 0.95rem;
  }
</style>

<div class="dashboard-container animate-fade-in">
  <div>
    <h1 style="font-size: 2rem; font-weight: 600; color: var(--text-primary); margin-bottom: 0.5rem;">Proxmox Cluster Status</h1>
    <p style="color: var(--text-secondary); font-size: 1rem;">Real-time cluster resources and active instances.</p>
  </div>

  {#if errorMsg}
    <div class="alert-banner">{errorMsg}</div>
  {/if}

  <div class="stats-grid">
    <div class="flat-card">
      <h3>CPU Usage</h3>
      <div class="stat-value">{stats.cpu}%</div>
      <div class="progress-bar">
        <div class="progress-fill" style="width: {stats.cpu}%"></div>
      </div>
    </div>
    
    <div class="flat-card">
      <h3>RAM Usage</h3>
      <div class="stat-value">{stats.ram}%</div>
      <div class="progress-bar">
        <div class="progress-fill" style="width: {stats.ram}%; background: {stats.ram > 80 ? 'var(--danger-color)' : 'var(--primary-color)'}"></div>
      </div>
    </div>

    <div class="flat-card">
      <h3>Storage Space</h3>
      <div class="stat-value">{stats.disk}%</div>
      <div class="progress-bar">
        <div class="progress-fill" style="width: {stats.disk}%"></div>
      </div>
    </div>

    <div class="flat-card">
      <h3>Active LXCs</h3>
      <div class="stat-value">{stats.activeLxcs}</div>
      <div class="subtitle">Online & Running</div>
    </div>
  </div>
</div>
