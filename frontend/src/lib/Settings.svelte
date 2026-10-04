<script lang="ts">
  import { onMount } from 'svelte';
  import PortalUpdates from './PortalUpdates.svelte';

  let tplStorage = 'local';
  let rootfsStorage = 'local-lvm';
  let saved = false;

  onMount(() => {
    tplStorage = localStorage.getItem('hostable_tpl_storage') || 'local';
    rootfsStorage = localStorage.getItem('hostable_rootfs_storage') || 'local-lvm';
  });

  function saveSettings() {
    localStorage.setItem('hostable_tpl_storage', tplStorage);
    localStorage.setItem('hostable_rootfs_storage', rootfsStorage);
    saved = true;
    setTimeout(() => saved = false, 3000);
  }
</script>

<style>
  .settings-container {
    max-width: 600px;
  }

  .flat-card {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 2rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  }

  .form-group {
    margin-bottom: 1.5rem;
  }

  .form-group label {
    display: block;
    color: #cbd5e1;
    font-size: 0.95rem;
    margin-bottom: 0.5rem;
    font-weight: 500;
  }

  .form-group input {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    color: #f8fafc;
    border-radius: 6px;
    padding: 0.8rem;
    font-size: 1rem;
    box-sizing: border-box;
  }

  .form-group input:focus {
    outline: none;
    border-color: #38bdf8;
  }

  .btn-primary {
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 6px;
    padding: 0.8rem 1.5rem;
    font-weight: 600;
    cursor: pointer;
    font-size: 1rem;
    transition: background 0.2s;
  }

  .btn-primary:hover {
    background: #0ea5e9;
  }
</style>

<div class="settings-container animate-fade-in">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">Settings</h1>
    <p style="color: #94a3b8; font-size: 1rem;">Configure global defaults for Hostable deployments.</p>
  </div>

  <div class="flat-card">
    <div class="form-group">
      <label for="tpl">Proxmox Template Storage Pool</label>
      <input id="tpl" type="text" bind:value={tplStorage} placeholder="e.g. local" />
      <p style="color: #64748b; font-size: 0.85rem; margin-top: 0.4rem;">Where the downloaded .tar.xz container templates will be saved.</p>
    </div>

    <div class="form-group">
      <label for="rootfs">Proxmox RootFS Storage Pool</label>
      <input id="rootfs" type="text" bind:value={rootfsStorage} placeholder="e.g. local-lvm" />
      <p style="color: #64748b; font-size: 0.85rem; margin-top: 0.4rem;">Where the actual LXC container virtual disks will be created.</p>
    </div>

    <div style="display: flex; align-items: center; gap: 1rem; margin-top: 2rem;">
      <button class="btn-primary" on:click={saveSettings}>Save Settings</button>
      {#if saved}
        <span style="color: #4ade80; font-size: 0.9rem;">✔ Saved successfully!</span>
      {/if}
    </div>
  </div>
  <PortalUpdates />
</div>
