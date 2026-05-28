<script lang="ts">
  import { onMount } from 'svelte';

  type ProxyRule = { id?: number, domain: string, target_ip: string, target_port: number, container_id?: number, auth_enabled?: boolean };
  let rules: ProxyRule[] = [];
  
  let newDomain = "";
  let newIp = "";
  let newPort = 80;
  let newAuthEnabled = false;

  let isSubmitting = false;
  let statusMsg = "";

  async function fetchRules() {
    try {
      const res = await fetch('/api/proxy-rules');
      if (res.ok) {
        rules = await res.json();
      }
    } catch (err) {
      console.error(err);
      // Fallback
      rules = [
        { id: 1, domain: "pihole.local", target_ip: "10.0.0.5", target_port: 80, container_id: 103, auth_enabled: false },
        { id: 2, domain: "jellyfin.local", target_ip: "10.0.0.6", target_port: 8096, container_id: 104, auth_enabled: true },
      ];
    }
  }

  async function addRule() {
    isSubmitting = true;
    statusMsg = "";
    try {
      const res = await fetch('/api/proxy-rules', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          domain: newDomain,
          target_ip: newIp,
          target_port: newPort,
          auth_enabled: newAuthEnabled,
        })
      });
      if (res.ok) {
        statusMsg = "Rule added successfully!";
        rules = [...rules, { domain: newDomain, target_ip: newIp, target_port: newPort, auth_enabled: newAuthEnabled }];
        newDomain = ""; newIp = ""; newPort = 80; newAuthEnabled = false;
      } else {
        statusMsg = "Failed to add rule.";
      }
    } catch (err) {
      statusMsg = "Error: " + err;
    } finally {
      isSubmitting = false;
    }
  }

  onMount(() => {
    fetchRules();
  });
</script>

<style>
  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.5rem 2rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  }

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

  .flat-btn-primary {
    background: #38bdf8;
    border: none;
    color: #0f172a;
    border-radius: 6px;
    padding: 0.6rem 1.5rem;
    font-size: 0.95rem;
    font-weight: 600;
    cursor: pointer;
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
      <div style="margin-top: 1rem; color: #4ade80; font-size: 0.95rem; font-weight: 500;">{statusMsg}</div>
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
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</div>
