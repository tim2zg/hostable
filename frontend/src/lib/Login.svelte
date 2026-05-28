<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  const dispatch = createEventDispatcher();
  let token = "";
  let isLoggingIn = false;
  let errorMsg = "";

  async function handleLogin() {
    if (!token.trim()) return;
    isLoggingIn = true;
    errorMsg = "";

    try {
      // Temporarily store token for verification request
      localStorage.setItem('hostable_token', token);
      
      const res = await fetch('/api/verify', {
        headers: { 'Authorization': `Bearer ${token}` }
      });

      if (res.ok) {
        dispatch('login');
      } else {
        localStorage.removeItem('hostable_token');
        errorMsg = "Invalid API Token. Please check and try again.";
      }
    } catch (err) {
      localStorage.removeItem('hostable_token');
      errorMsg = "Network error. Unable to verify token.";
    } finally {
      isLoggingIn = false;
    }
  }
</script>

<style>
  .login-container {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #0f172a;
    font-family: 'Outfit', sans-serif;
  }

  .flat-login-card {
    width: 100%;
    max-width: 400px;
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 2.5rem;
    box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .login-header {
    text-align: center;
  }

  .login-header h2 {
    font-size: 1.8rem;
    color: #f8fafc;
    margin-bottom: 0.5rem;
  }

  .login-header p {
    font-size: 0.95rem;
    color: #94a3b8;
  }

  .input-group {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .input-label {
    font-size: 0.9rem;
    color: #cbd5e1;
    font-weight: 500;
  }

  .flat-input {
    width: 100%;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    color: #e2e8f0;
    padding: 0.75rem 1rem;
    font-size: 1rem;
    outline: none;
    transition: border-color 0.2s;
  }

  .flat-input:focus {
    border-color: #38bdf8;
  }

  .flat-btn {
    width: 100%;
    background: #38bdf8;
    color: #0f172a;
    border: none;
    border-radius: 6px;
    padding: 0.75rem;
    font-size: 1rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.2s;
  }

  .flat-btn:hover:not(:disabled) {
    background: #0ea5e9;
  }

  .flat-btn:disabled {
    background: #475569;
    color: #94a3b8;
    cursor: not-allowed;
  }

  .error-banner {
    padding: 0.75rem;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid #ef4444;
    color: #f87171;
    border-radius: 6px;
    font-size: 0.9rem;
    text-align: center;
  }
</style>

<div class="login-container">
  <div class="flat-login-card">
    <div class="login-header">
      <h2>Hostable.</h2>
      <p>Enter your secure Admin API Token to login</p>
    </div>

    {#if errorMsg}
      <div class="error-banner">{errorMsg}</div>
    {/if}

    <div class="input-group">
      <label class="input-label" for="token">API Token</label>
      <input 
        id="token"
        type="password" 
        class="flat-input" 
        placeholder="hst_..." 
        bind:value={token} 
        on:keydown={(e) => e.key === 'Enter' && handleLogin()}
        disabled={isLoggingIn}
      />
    </div>

    <button class="flat-btn" on:click={handleLogin} disabled={isLoggingIn || !token.trim()}>
      {isLoggingIn ? 'Verifying...' : 'Unlock Dashboard'}
    </button>
  </div>
</div>
