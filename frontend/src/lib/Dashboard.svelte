<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet } from './api';
  import Chart from 'chart.js/auto';

  let stats = {
    cpu: 0,
    ram: 0,
    disk: 0,
    activeLxcs: 0
  };
  let errorMsg = "";
  let interval: any;
  
  let chartCanvas: HTMLCanvasElement;
  let chartInstance: any;

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

  async function fetchRrdData() {
    if (document.hidden) return;
    try {
      const res = await apiGet('/node/rrddata');
      if (res && res.data) {
        updateChart(res.data);
      }
    } catch (err) {
      console.error("Failed to fetch rrd data", err);
    }
  }

  function updateChart(rrdData: any[]) {
    if (!chartCanvas) return;
    
    // Filter out null/empty data points
    const validData = rrdData.filter(d => d.cpu !== undefined && d.cpu !== null);
    
    const labels = validData.map(d => {
      const date = new Date(d.time * 1000);
      return `${date.getHours()}:${date.getMinutes().toString().padStart(2, '0')}`;
    });
    
    const cpuData = validData.map(d => (d.cpu * 100).toFixed(1));
    const ramData = validData.map(d => {
      if (d.memused && d.memtotal) {
        return ((d.memused / d.memtotal) * 100).toFixed(1);
      }
      return 0;
    });

    if (chartInstance) {
      chartInstance.data.labels = labels;
      chartInstance.data.datasets[0].data = cpuData;
      chartInstance.data.datasets[1].data = ramData;
      chartInstance.update('none'); // Update without full animation
    } else {
      chartInstance = new Chart(chartCanvas, {
        type: 'line',
        data: {
          labels: labels,
          datasets: [
            {
              label: 'CPU Usage (%)',
              data: cpuData,
              borderColor: '#38bdf8',
              backgroundColor: 'rgba(56, 189, 248, 0.15)',
              borderWidth: 2,
              tension: 0.4,
              fill: true,
              pointRadius: 0,
              pointHitRadius: 10
            },
            {
              label: 'RAM Usage (%)',
              data: ramData,
              borderColor: '#c084fc',
              backgroundColor: 'rgba(192, 132, 252, 0.15)',
              borderWidth: 2,
              tension: 0.4,
              fill: true,
              pointRadius: 0,
              pointHitRadius: 10
            }
          ]
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: {
            mode: 'index',
            intersect: false,
          },
          plugins: {
            legend: {
              labels: { color: '#94a3b8', font: { family: 'inherit', size: 13 } }
            },
            tooltip: {
              backgroundColor: 'rgba(15, 23, 42, 0.9)',
              titleColor: '#f8fafc',
              bodyColor: '#e2e8f0',
              borderColor: '#334155',
              borderWidth: 1
            }
          },
          scales: {
            y: {
              min: 0,
              max: 100,
              grid: { color: '#1e293b' },
              ticks: { color: '#64748b', font: { family: 'inherit' } },
              border: { dash: [4, 4] }
            },
            x: {
              grid: { display: false },
              ticks: { color: '#64748b', maxTicksLimit: 8, font: { family: 'inherit' } }
            }
          }
        }
      });
    }
  }

  onMount(() => {
    fetchStats();
    fetchRrdData();
    interval = setInterval(() => {
      fetchStats();
      fetchRrdData();
    }, 15000);
    document.addEventListener('visibilitychange', fetchStats);
  });

  onDestroy(() => {
    clearInterval(interval);
    document.removeEventListener('visibilitychange', fetchStats);
    if (chartInstance) chartInstance.destroy();
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

  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 1.5rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
    transition: transform 0.2s, border-color 0.2s, box-shadow 0.2s;
  }
  
  .flat-card:hover {
    transform: translateY(-2px);
    border-color: #475569;
    box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05);
  }

  .flat-card h3 {
    font-size: 1.1rem;
    color: #94a3b8;
    font-weight: 500;
    margin-bottom: 0.5rem;
  }

  .stat-value {
    font-size: 2.5rem;
    font-weight: 700;
    color: #f8fafc;
    margin-bottom: 1rem;
  }

  .subtitle {
    color: #10b981;
    font-size: 0.95rem;
    font-weight: 500;
  }

  .progress-bar {
    width: 100%;
    height: 6px;
    background: #0f172a;
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: #38bdf8;
    transition: width 0.5s ease;
  }

  .alert-banner {
    padding: 1rem;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid #ef4444;
    color: #f87171;
    border-radius: 8px;
    font-size: 0.95rem;
  }

  .chart-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 12px;
    padding: 1.5rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
    height: 400px;
    display: flex;
    flex-direction: column;
  }

  .chart-card h3 {
    font-size: 1.2rem;
    color: #f8fafc;
    font-weight: 600;
    margin-bottom: 1rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  
  .chart-container {
    flex-grow: 1;
    position: relative;
    width: 100%;
  }
</style>

<div class="dashboard-container animate-fade-in">
  <div>
    <h1 style="font-size: 2.2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Cluster Dashboard</h1>
    <p style="color: #94a3b8; font-size: 1.05rem;">Real-time telemetry and resource allocation.</p>
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
        <div class="progress-fill" style="width: {stats.ram}%; background: {stats.ram > 80 ? '#ef4444' : '#38bdf8'}"></div>
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
      <h3>Active Containers</h3>
      <div class="stat-value" style="color: #38bdf8;">{stats.activeLxcs}</div>
      <div class="subtitle">● Online & Running</div>
    </div>
  </div>

  <!-- Historical Telemetry Graph -->
  <div class="chart-card">
    <h3>📈 Node Telemetry (Last Hour)</h3>
    <div class="chart-container">
      <canvas bind:this={chartCanvas}></canvas>
    </div>
  </div>
</div>
