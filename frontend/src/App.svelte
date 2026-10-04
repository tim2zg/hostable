<script lang="ts">
  import './app.css';
  import './lib/platform.css';
  import DatabaseManager from './lib/DatabaseManager.svelte';
  import Jobs from './lib/Jobs.svelte';
  import Workloads from './lib/Workloads.svelte';
  import Operations from './lib/Operations.svelte';
  import Converter from './lib/Converter.svelte';
  import Catalog from './lib/Catalog.svelte';
  import SecureWebGateway from './lib/SecureWebGateway.svelte';
  import Login from './lib/Login.svelte';
  import Settings from './lib/Settings.svelte';
  import ToastRenderer from './lib/ToastRenderer.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { apiGet } from './lib/api';

  let currentView = 'dashboard';
  let isAuthenticated = false;
  let isChecking = true;
  let sidebarOpen = false;

  function logout() {
    localStorage.removeItem('hostable_token');
    isAuthenticated = false;
    currentView = 'dashboard';
  }

  onMount(async () => {
    window.addEventListener('hostable-logout', logout);
    const token = localStorage.getItem('hostable_token');
    if (token) {
      try {
        await apiGet('/verify');
        isAuthenticated = true;
      } catch (e) {
        localStorage.removeItem('hostable_token');
        isAuthenticated = false;
      }
    }
    isChecking = false;
  });

  onDestroy(() => {
    window.removeEventListener('hostable-logout', logout);
  });

  function onLogin() {
    isAuthenticated = true;
  }

  function setView(view: string) {
    currentView = view;
    sidebarOpen = false;
  }
</script>

{#if isChecking}
  <div class="loading-screen">
    <div class="loading-spinner"></div>
    <p>Loading Hostable...</p>
  </div>
{:else if !isAuthenticated}
  <Login on:login={onLogin} />
{:else}
<div class="app-layout">
  <button class="mobile-menu-btn" on:click={() => sidebarOpen = !sidebarOpen} aria-label="Toggle menu">
    {sidebarOpen ? '✕' : '☰'}
  </button>

  <aside class="sidebar" class:sidebar-open={sidebarOpen}>
    <div class="sidebar-brand">Hostable.</div>
    <nav>
      <button class="nav-item {currentView === 'dashboard' ? 'active' : ''}" on:click={() => setView('dashboard')}>📊 Dashboard</button>
      <button class="nav-item {currentView === 'lxc' ? 'active' : ''}" on:click={() => setView('lxc')}>📦 LXC Manager</button>
      <button class="nav-item {currentView === 'workloads' ? 'active' : ''}" on:click={() => setView('workloads')}>Applications and updates</button>
      <button class="nav-item {currentView === 'operations' ? 'active' : ''}" on:click={() => setView('operations')}>Health and recovery</button>
      <button class="nav-item {currentView === 'converter' ? 'active' : ''}" on:click={() => setView('converter')}>🚀 Deploy Wizard</button>
      <button class="nav-item {currentView === 'catalog' ? 'active' : ''}" on:click={() => setView('catalog')}>🛒 Container recipes</button>
      <button class="nav-item {currentView === 'databases' ? 'active' : ''}" on:click={() => setView('databases')}>Databases</button>
      <button class="nav-item {currentView === 'jobs' ? 'active' : ''}" on:click={() => setView('jobs')}>Operation history</button>
      <button class="nav-item {currentView === 'secureweb' ? 'active' : ''}" on:click={() => setView('secureweb')}>🛡️ SecureWeb Gateway</button>
      <button class="nav-item {currentView === 'settings' ? 'active' : ''}" on:click={() => setView('settings')}>⚙️ Settings</button>
    </nav>
    <div class="sidebar-footer">
      <button class="nav-item logout-btn" on:click={logout}>🚪 Logout</button>
    </div>
  </aside>

  <main class="main-content">
    {#if currentView === 'dashboard'}
      {#await import('./lib/Dashboard.svelte')}<p>Loading dashboard...</p>{:then view}<svelte:component this={view.default} />{:catch error}<p>Unable to load dashboard: {error.message}</p>{/await}
    {:else if currentView === 'lxc'}
      {#await import('./lib/LxcManager.svelte')}<p>Loading container manager...</p>{:then view}<svelte:component this={view.default} />{:catch error}<p>Unable to load container manager: {error.message}</p>{/await}
    {:else if currentView === 'converter'}
      <Converter />
    {:else if currentView === 'catalog'}
      <Catalog />
    {:else if currentView === 'databases'}
      <DatabaseManager />
    {:else if currentView === 'jobs'}
      <Jobs />
    {:else if currentView === 'workloads'}
      <Workloads />
    {:else if currentView === 'operations'}
      <Operations />
    {:else if currentView === 'secureweb'}
      <SecureWebGateway />
    {:else if currentView === 'settings'}
      <Settings />
    {/if}
  </main>
</div>
<ToastRenderer />
{/if}
