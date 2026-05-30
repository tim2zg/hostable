<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost } from './api';

  let tables: string[] = [];
  let sqlQuery = "";
  let isExecuting = false;
  let statusMessage = "";
  let statusType: 'success' | 'error' | '' = '';
  let rowsAffected: number | null = null;

  async function fetchSchema() {
    try {
      const data = await apiGet('/db/schema');
      if (data.status === 'ok' || data.status === 'mock') {
          tables = data.tables || [];
        }
    } catch (err) {
      console.error(err);
      statusMessage = "Network error loading database schema.";
      statusType = 'error';
    }
  }

  async function executeSQL() {
    if (!sqlQuery.trim()) return;
    isExecuting = true;
    statusMessage = "";
    statusType = '';
    rowsAffected = null;

    try {
      const data = await apiPost('/db/execute', { sql: sqlQuery });

      if (data.status === 'ok') {
          statusMessage = "Query executed successfully!";
          statusType = 'success';
          rowsAffected = data.rows_affected;
          fetchSchema(); // Refresh tables list in case they created/dropped a table
        } else if (data.status === 'mock') {
          statusMessage = `Mock Mode: query would execute successfully. (${data.message})`;
          statusType = 'success';
        }
    } catch (err) {
      statusMessage = "Error: " + err;
      statusType = 'error';
    } finally {
      isExecuting = false;
    }
  }

  function selectTemplate(tableName: string) {
    sqlQuery = `SELECT * FROM ${tableName} LIMIT 10;`;
  }

  onMount(() => {
    fetchSchema();
  });
</script>

<style>
  .db-container {
    display: grid;
    grid-template-columns: 240px 1fr;
    gap: 1.5rem;
    height: calc(100vh - 12rem);
    background: #0f172a;
    color: #e2e8f0;
  }

  @media (max-width: 800px) {
    .db-container {
      grid-template-columns: 1fr;
      height: auto;
    }
  }

  .flat-sidebar {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    overflow-y: auto;
  }

  .flat-sidebar h3 {
    font-size: 1.1rem;
    color: #38bdf8;
    margin-bottom: 0.5rem;
    border-bottom: 1px solid #334155;
    padding-bottom: 0.5rem;
    font-weight: 500;
  }

  .table-item {
    padding: 0.6rem 0.8rem;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    cursor: pointer;
    font-family: monospace;
    font-size: 0.9rem;
    color: #cbd5e1;
    transition: all 0.2s ease;
    text-align: left;
  }

  .table-item:hover {
    border-color: #38bdf8;
    background: #1e293b;
    color: #38bdf8;
  }

  .main-panel {
    display: flex;
    flex-direction: column;
    gap: 1.2rem;
    overflow-y: auto;
  }

  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  }

  .flat-card h2 {
    font-size: 1.3rem;
    color: #f8fafc;
    font-weight: 600;
  }

  .sql-textarea {
    width: 100%;
    min-height: 140px;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    color: #e2e8f0;
    font-family: monospace;
    padding: 1rem;
    font-size: 0.95rem;
    resize: vertical;
    outline: none;
  }

  .sql-textarea:focus {
    border-color: #38bdf8;
  }

  .action-bar {
    display: flex;
    justify-content: flex-end;
    gap: 1rem;
  }

  .flat-btn-primary {
    background: #38bdf8;
    color: #0f172a;
    font-weight: 600;
    border: none;
    border-radius: 6px;
    padding: 0.6rem 1.5rem;
    cursor: pointer;
    font-size: 0.95rem;
    transition: background 0.2s;
  }

  .flat-btn-primary:hover:not(:disabled) {
    background: #0ea5e9;
  }

  .flat-btn-primary:disabled {
    background: #475569;
    color: #94a3b8;
    cursor: not-allowed;
  }

  .banner {
    padding: 0.8rem 1rem;
    border-radius: 6px;
    font-size: 0.95rem;
    font-weight: 500;
  }

  .banner.success {
    background: rgba(34, 197, 94, 0.15);
    border: 1px solid #22c55e;
    color: #4ade80;
  }

  .banner.error {
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid #ef4444;
    color: #f87171;
  }

  .no-tables {
    font-size: 0.9rem;
    color: #64748b;
    text-align: center;
    padding: 1rem 0;
  }
</style>

<div class="db-container animate-fade-in">
  <!-- Left Side: Table List Browser -->
  <aside class="flat-sidebar">
    <h3>Active Tables</h3>
    {#if tables.length === 0}
      <div class="no-tables">No public tables found</div>
    {:else}
      {#each tables as table}
        <button class="table-item" on:click={() => selectTemplate(table)}>
          📁 {table}
        </button>
      {/each}
    {/if}
  </aside>

  <!-- Right Side: SQL Executor -->
  <main class="main-panel">
    <div class="flat-card">
      <h2>Database Provisioning Console</h2>
      <p style="color: #94a3b8; font-size: 0.9rem;">
        Direct database query execution panel. Destructive commands targeting Hostable's core tables (`users`, `proxy_rules`) are automatically blocked for security.
      </p>

      <textarea
        class="sql-textarea"
        placeholder="-- e.g. CREATE TABLE app_config (id SERIAL PRIMARY KEY, domain TEXT);\nSELECT * FROM pg_tables WHERE schemaname = 'public';"
        bind:value={sqlQuery}
      ></textarea>

      <div class="action-bar">
        <button class="flat-btn-primary" on:click={executeSQL} disabled={isExecuting || !sqlQuery.trim()}>
          {isExecuting ? 'Executing...' : 'Run Query'}
        </button>
      </div>

      {#if statusMessage}
        <div class="banner {statusType}">
          {statusMessage}
        </div>
      {/if}

      {#if rowsAffected !== null}
        <div style="font-family: monospace; font-size: 0.9rem; color: #38bdf8;">
          Rows affected: {rowsAffected}
        </div>
      {/if}
    </div>
  </main>
</div>
