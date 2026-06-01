<script lang="ts">
  import { toasts } from './toast';
  import { fade, fly } from 'svelte/transition';
</script>

<style>
  .toast-container {
    position: fixed;
    bottom: 2rem;
    right: 2rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
    z-index: 9999;
    pointer-events: none; /* Let clicks pass through the container */
  }

  .toast {
    pointer-events: auto; /* Re-enable clicks for individual toasts */
    min-width: 300px;
    background: #1e293b;
    border-radius: 8px;
    padding: 1rem 1.2rem;
    display: flex;
    align-items: center;
    gap: 1rem;
    box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.5), 0 4px 6px -2px rgba(0, 0, 0, 0.25);
    border-left: 4px solid #38bdf8;
    color: #f8fafc;
    font-size: 0.95rem;
    font-weight: 500;
  }

  .toast.success { border-left-color: #10b981; }
  .toast.error { border-left-color: #ef4444; }
  .toast.info { border-left-color: #38bdf8; }

  .icon {
    font-size: 1.2rem;
  }

  .message {
    flex-grow: 1;
  }

  .close-btn {
    background: none;
    border: none;
    color: #94a3b8;
    cursor: pointer;
    font-size: 1.2rem;
    padding: 0;
    margin: 0;
    line-height: 1;
  }

  .close-btn:hover {
    color: #e2e8f0;
  }
</style>

<div class="toast-container">
  {#each $toasts as t (t.id)}
    <div 
      class="toast {t.type}" 
      in:fly="{{ y: 20, duration: 300 }}" 
      out:fade="{{ duration: 200 }}"
    >
      <span class="icon">
        {#if t.type === 'success'}✅{/if}
        {#if t.type === 'error'}❌{/if}
        {#if t.type === 'info'}ℹ️{/if}
      </span>
      <span class="message">{t.message}</span>
      <button class="close-btn" on:click={() => toasts.remove(t.id)}>×</button>
    </div>
  {/each}
</div>
