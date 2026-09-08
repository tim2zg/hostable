<script lang="ts">
  import './app.css';
  import Dashboard from './lib/Dashboard.svelte';
  import LxcManager from './lib/LxcManager.svelte';
  import Converter from './lib/Converter.svelte';
  import Catalog from './lib/Catalog.svelte';
  import SecureWebGateway from './lib/SecureWebGateway.svelte';
  import LogViewer from './lib/LogViewer.svelte';
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
      <button class="nav-item {currentView === 'converter' ? 'active' : ''}" on:click={() => setView('converter')}>🚀 Deploy Wizard</button>
      <button class="nav-item {currentView === 'catalog' ? 'active' : ''}" on:click={() => setView('catalog')}>🛒 Catalog (1-Click)</button>
      <button class="nav-item {currentView === 'secureweb' ? 'active' : ''}" on:click={() => setView('secureweb')}>🛡️ SecureWeb Gateway</button>
      <button class="nav-item {currentView === 'logs' ? 'active' : ''}" on:click={() => setView('logs')}>📋 Real-Time Logs</button>
      <button class="nav-item {currentView === 'settings' ? 'active' : ''}" on:click={() => setView('settings')}>⚙️ Settings</button>
    </nav>
    <div class="sidebar-footer">
      <button class="nav-item logout-btn" on:click={logout}>🚪 Logout</button>
    </div>
  </aside>

  <main class="main-content">
    {#if currentView === 'dashboard'}
      <Dashboard />
    {:else if currentView === 'lxc'}
      <LxcManager />
    {:else if currentView === 'converter'}
      <Converter />
    {:else if currentView === 'catalog'}
      <Catalog />
    {:else if currentView === 'secureweb'}
      <SecureWebGateway />
    {:else if currentView === 'logs'}
      <LogViewer />
    {:else if currentView === 'settings'}
      <Settings />
    {/if}
  </main>
</div>
<ToastRenderer />
{/if}
