<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPost, apiDelete } from './api';

  type ProxyRule = { id?: number, domain: string, target_ip: string, target_port: number, container_id?: number, auth_enabled?: boolean };
  let rules: ProxyRule[] = [];
  
  let newDomain = "";
  let newIp = "";
  let newPort = 80;
  let newAuthEnabled = false;

  let isSubmitting = false;
  let statusMsg = "";
  let statusError = false;

  async function fetchRules() {
    try {
      rules = await apiGet('/proxy-rules');
    } catch (err) {
      console.error(err);
      statusMsg = "Failed to load proxy rules.";
      statusError = true;
      rules = [];
    }
  }

  async function addRule() {
    isSubmitting = true;
    statusMsg = "";
    statusError = false;
    try {
      await apiPost('/proxy-rules', {
        domain: newDomain,
        target_ip: newIp,
        target_port: newPort,
        auth_enabled: newAuthEnabled,
      });
      statusMsg = "Rule added successfully!";
      statusError = false;
      newDomain = ""; newIp = ""; newPort = 80; newAuthEnabled = false;
      await fetchRules();
    } catch (err) {
      statusMsg = "Error: " + err;
      statusError = true;
    } finally {
      isSubmitting = false;
    }
  }

  async function deleteRule(id: number | undefined) {
    if (!id) return;
    if (!confirm(`Are you sure you want to delete this rule?`)) return;
    try {
      await apiDelete(`/proxy-rules/${id}`);
      await fetchRules();
    } catch (err) {
      console.error(err);
      statusMsg = "Failed to delete rule.";
      statusError = true;
    }
  }

  onMount(() => {
    fetchRules();
  });
</script>

<style>
  .form-grid {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
    align-items: flex-end;
  }

  .input-group {
    flex: 1;
    min-width: 150px;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .input-label {
    color: #cbd5e1;
    font-size: 0.9rem;
    font-weight: 500;
  }

  .flat-input {
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    color: #e2e8f0;
    padding: 0.6rem 0.8rem;
    font-size: 0.95rem;
    outline: none;
  }

  .flat-input:focus {
    border-color: #38bdf8;
  }

  .rules-table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 1.5rem;
  }

  .rules-table th, .rules-table td {
    padding: 0.8rem 1rem;
    text-align: left;
    border-bottom: 1px solid #334155;
  }

  .rules-table th {
    color: #94a3b8;
    font-weight: 500;
    font-size: 0.9rem;
  }

  .rules-table td {
    color: #e2e8f0;
  }

  .badge-protected {
    background: rgba(168, 85, 247, 0.15);
    color: #c084fc;
    border: 1px solid #a855f7;
    padding: 0.2rem 0.6rem;
    border-radius: 12px;
    font-size: 0.8rem;
    font-weight: 600;
  }

  .badge-public {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
    border: 1px solid #64748b;
    padding: 0.2rem 0.6rem;
    border-radius: 12px;
    font-size: 0.8rem;
    font-weight: 600;
  }
</style>

<div class="animate-fade-in">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Proxy Edge Routing</h1>
    <p style="color: #94a3b8; font-size: 1rem;">Map custom domain hostnames securely to internal LXC endpoints with optional Authelia identity checks.</p>
  </div>

  <!-- Create Route Panel -->
  <div class="flat-card" style="margin-bottom: 2rem;">
    <h3 style="color: #cbd5e1; margin-bottom: 1.2rem; font-size: 1.1rem; font-weight: 500;">Add New Reverse Proxy Route</h3>
    <div class="form-grid">
      <div class="input-group" style="flex: 2;">
        <label class="input-label" for="domain">Domain / Subdomain</label>
        <input id="domain" type="text" class="flat-input" placeholder="e.g. app.domain.com" bind:value={newDomain} />
      </div>
      <div class="input-group" style="flex: 1.5;">
        <label class="input-label" for="ip">Target Internal IP</label>
        <input id="ip" type="text" class="flat-input" placeholder="e.g. 10.0.0.5" bind:value={newIp} />
      </div>
      <div class="input-group" style="flex: 0.5;">
        <label class="input-label" for="port">Port</label>
        <input id="port" type="number" class="flat-input" placeholder="80" bind:value={newPort} />
      </div>
      <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.6rem; margin-left: 0.5rem;">
        <input id="auth" type="checkbox" bind:checked={newAuthEnabled} style="cursor: pointer; width: 1.1rem; height: 1.1rem; accent-color: #38bdf8;" />
        <label for="auth" style="cursor: pointer; color: #cbd5e1; font-size: 0.9rem;">Require Authelia Auth</label>
      </div>
      <button class="flat-btn-primary" on:click={addRule} disabled={isSubmitting || !newDomain || !newIp}>
        {isSubmitting ? 'Adding...' : 'Add Route'}
      </button>
    </div>
    {#if statusMsg}
      <div style="margin-top: 1rem; color: {statusError ? '#f87171' : '#4ade80'}; font-size: 0.95rem; font-weight: 500;">{statusMsg}</div>
    {/if}
  </div>

  <!-- Active Routes Table -->
  <div class="flat-card">
    <h3 style="color: #cbd5e1; margin-bottom: 1rem; font-size: 1.1rem; font-weight: 500;">Active Routing Mapping Rules</h3>
    {#if rules.length === 0}
      <p style="color: #94a3b8; font-size: 0.95rem;">No proxy routing rules defined.</p>
    {:else}
      <table class="rules-table">
        <thead>
          <tr>
            <th>Domain Hostname</th>
            <th>LXC Internal Target</th>
            <th>Authelia Zero-Trust</th>
            <th>Status</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each rules as rule}
            <tr>
              <td style="font-weight: 500; color: #f8fafc;">{rule.domain}</td>
              <td style="font-family: monospace; color: #38bdf8;">{rule.target_ip}:{rule.target_port}</td>
              <td>
                {#if rule.auth_enabled}
                  <span class="badge-protected">Protected</span>
                {:else}
                  <span class="badge-public">Public (Open)</span>
                {/if}
              </td>
              <td><span style="color: #4ade80; font-weight: 500;">Active</span></td>
              <td>
                <button class="flat-btn-danger" style="padding: 0.2rem 0.5rem; font-size: 0.8rem;" on:click={() => deleteRule(rule.id)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</div>
