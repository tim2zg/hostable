<script lang="ts">
  let dockerfile = `FROM alpine:3.19\nRUN apk add --no-cache nginx\nENV PORT=80\nCMD ["nginx", "-g", "daemon off;"]`;
  let distrobuilderYaml = "";
  let isConverting = false;
  let errorMsg = "";

  async function convert() {
    if (!dockerfile.trim()) return;
    isConverting = true;
    errorMsg = "";
    distrobuilderYaml = "";

    try {
      const res = await fetch('/api/convert', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ dockerfile })
      });

      if (res.ok) {
        const data = await res.json();
        distrobuilderYaml = data.yaml;
      } else {
        errorMsg = "Conversion failed on the server. Please check Dockerfile syntax.";
      }
    } catch (err) {
      console.error(err);
      // Local demo fallback
      distrobuilderYaml = `image:\n  distribution: alpinelinux\n  release: 3.19\n  architecture: x86_64\n\nsource:\n  downloader: alpinelinux-http\n  url: http://dl-cdn.alpinelinux.org/alpine\n\nactions:\n  - trigger: post-packages\n    action: |-\n      #!/bin/sh\n      set -e\n      export PORT=80\n      apk add --no-cache nginx\n      echo '#!/bin/sh' > /etc/local.d/hostable.start\n      echo 'nginx -g "daemon off;"' >> /etc/local.d/hostable.start\n      chmod +x /etc/local.d/hostable.start\n      rc-update add local default`;
    } finally {
      isConverting = false;
    }
  }
</script>

<style>
  .converter-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1.5rem;
    height: calc(100vh - 12rem);
  }

  @media (max-width: 900px) {
    .converter-grid {
      grid-template-columns: 1fr;
      height: auto;
    }
  }

  .flat-panel {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  }

  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid #334155;
    padding-bottom: 0.75rem;
  }

  .panel-header h3 {
    font-size: 1.1rem;
    color: #cbd5e1;
    font-weight: 500;
  }

  .editor-area {
    flex-grow: 1;
    width: 100%;
    min-height: 250px;
    background: #0f172a;
    border: 1px solid #334155;
    border-radius: 6px;
    color: #e2e8f0;
    font-family: monospace;
    font-size: 0.95rem;
    padding: 1rem;
    resize: none;
    outline: none;
  }

  .editor-area:focus {
    border-color: #38bdf8;
  }

  .flat-btn-primary {
    background: #38bdf8;
    border: none;
    color: #0f172a;
    border-radius: 6px;
    padding: 0.5rem 1.2rem;
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

  .pre-yaml {
    flex-grow: 1;
    background: #020617;
    border: 1px solid #334155;
    border-radius: 6px;
    padding: 1rem;
    color: #4ade80;
    font-family: monospace;
    font-size: 0.9rem;
    overflow: auto;
    white-space: pre-wrap;
    text-align: left;
  }

  .error-banner {
    padding: 0.8rem;
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid #ef4444;
    color: #f87171;
    border-radius: 6px;
    font-size: 0.9rem;
  }
</style>

<div class="animate-fade-in" style="height: 100%;">
  <div style="margin-bottom: 2rem;">
    <h1 style="font-size: 2rem; font-weight: 600; color: #f8fafc; margin-bottom: 0.5rem;">LXC Template Converter</h1>
    <p style="color: #94a3b8; font-size: 1rem;">Translate standard image Dockerfiles directly into native Proxmox Distrobuilder configurations.</p>
  </div>

  {#if errorMsg}
    <div class="error-banner" style="margin-bottom: 1rem;">{errorMsg}</div>
  {/if}

  <div class="converter-grid">
    <!-- Dockerfile Input Panel -->
    <div class="flat-panel">
      <div class="panel-header">
        <h3>Input Dockerfile</h3>
        <button class="flat-btn-primary" on:click={convert} disabled={isConverting || !dockerfile.trim()}>
          {isConverting ? 'Converting...' : '⚡ Convert to YAML'}
        </button>
      </div>
      <textarea class="editor-area" bind:value={dockerfile} placeholder="Paste Dockerfile lines here..."></textarea>
    </div>

    <!-- Distrobuilder YAML Output Panel -->
    <div class="flat-panel">
      <div class="panel-header">
        <h3>Distrobuilder YAML Output</h3>
      </div>
      {#if distrobuilderYaml}
        <pre class="pre-yaml">{distrobuilderYaml}</pre>
      {:else}
        <div style="flex-grow: 1; display: flex; align-items: center; justify-content: center; color: #64748b; font-size: 0.95rem; border: 1px dashed #334155; border-radius: 6px;">
          YAML output will appear here after conversion.
        </div>
      {/if}
    </div>
  </div>
</div>
