<script lang="ts">
  import './app.css';
  import Dashboard from './lib/Dashboard.svelte';
  import LxcManager from './lib/LxcManager.svelte';
  import Converter from './lib/Converter.svelte';
  import Catalog from './lib/Catalog.svelte';
  import ProxyRouter from './lib/ProxyRouter.svelte';
  import LogViewer from './lib/LogViewer.svelte';
  import DatabaseManager from './lib/DatabaseManager.svelte';
  import Login from './lib/Login.svelte';
  import Settings from './lib/Settings.svelte';
  import { onMount } from 'svelte';

  let currentView = 'dashboard';
  let isAuthenticated = false;
  let isChecking = true;

  onMount(async () => {
    const token = localStorage.getItem('hostable_token');
    
    // Setup global fetch interceptor for API calls
    const originalFetch = window.fetch;
    window.fetch = async function() {
      let [resource, config] = arguments;
      if (typeof resource === 'string' && resource.startsWith('/api/')) {
        config = config || {};
        config.headers = config.headers || {};
        const currentToken = localStorage.getItem('hostable_token');
        if (currentToken) {
          config.headers['Authorization'] = `Bearer ${currentToken}`;
        }
      }
      return originalFetch(resource, config);
    };

    if (token) {
      try {
        const res = await fetch('/api/verify');
        if (res.ok) {
          isAuthenticated = true;
        } else {
          localStorage.removeItem('hostable_token');
        }
      } catch (e) {
        // Network error, assume token is valid for offline
        isAuthenticated = true;
      }
    }
    isChecking = false;
  });

  function onLogin() {
    isAuthenticated = true;
  }

  function setView(view: string) {
    currentView = view;
  }
</script>

{#if isChecking}
  <div style="height: 100vh; display: flex; align-items: center; justify-content: center; color: var(--text-secondary); background: #0f172a;">
    Loading Hostable...
  </div>
{:else if !isAuthenticated}
  <Login on:login={onLogin} />
{:else}
<div class="app-layout">
  <!-- Sidebar -->
  <aside class="sidebar">
    <div class="sidebar-brand">
      Hostable.
    </div>
    <nav>
      <div 
        class="nav-item {currentView === 'dashboard' ? 'active' : ''}"
        on:click={() => setView('dashboard')}
      >
        Dashboard
      </div>
      <div 
        class="nav-item {currentView === 'lxc' ? 'active' : ''}"
        on:click={() => setView('lxc')}
      >
        LXC Manager
      </div>
      <div 
        class="nav-item {currentView === 'catalog' ? 'active' : ''}"
        on:click={() => setView('catalog')}
      >
        Catalog (1-Click)
      </div>
      <div 
        class="nav-item {currentView === 'converter' ? 'active' : ''}"
        on:click={() => setView('converter')}
      >
        Custom Deploy
      </div>
      <div 
        class="nav-item {currentView === 'proxy' ? 'active' : ''}"
        on:click={() => setView('proxy')}
      >
        Proxy Routing
      </div>
      <div 
        class="nav-item {currentView === 'logs' ? 'active' : ''}"
        on:click={() => setView('logs')}
      >
        Real-Time Logs
      </div>
      <div 
        class="nav-item {currentView === 'db' ? 'active' : ''}"
        on:click={() => setView('db')}
      >
        DB Provision
      </div>
      <div 
        class="nav-item {currentView === 'settings' ? 'active' : ''}"
        on:click={() => setView('settings')}
      >
        Settings
      </div>
    </nav>
  </aside>

  <!-- Main Content -->
  <main class="main-content">
    {#if currentView === 'dashboard'}
      <Dashboard />
    {:else if currentView === 'lxc'}
      <LxcManager />
    {:else if currentView === 'converter'}
      <Converter />
    {:else if currentView === 'catalog'}
      <Catalog />
    {:else if currentView === 'proxy'}
      <ProxyRouter />
    {:else if currentView === 'logs'}
      <LogViewer />
    {:else if currentView === 'db'}
      <DatabaseManager />
    {:else if currentView === 'settings'}
      <Settings />
    {/if}
  </main>
</div>
{/if}
