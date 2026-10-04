<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet, apiPost } from './api';
  import { infrastructure, watchJob, type Infrastructure, type TaskEvent } from './jobs';
  export let recipe: { image: string; hostname: string; port?: number; id?:string; version?:number; health?:any; volumes?: {container:string;size_gb:number}[] } | null = null;
  let image = recipe?.image || ''; let hostname = recipe?.hostname || ''; let vmid = 0; let node = '';
  let cpu = 2; let memory = 1024; let disk = 8; let storage = localStorage.getItem('hostable_rootfs_storage') || ''; let templateStorage = localStorage.getItem('hostable_tpl_storage') || ''; let bridge = ''; let ip = 'dhcp'; let gateway = '';
  let port = recipe?.port || 0; let expose = false; let domain = ''; let env = ''; let database = '';
  let volumes = (recipe?.volumes || []).map(v => ({ ...v, storage: '' }));
  let apps: any[] = []; let infra: Infrastructure = {nodes:[],root:[],templates:[],bridges:[]};
  let error = ''; let busy = false; let task = ''; let status = ''; let events: TaskEvent[] = []; let cancel: (() => void) | undefined;
  async function discover() {
    try { infra = await infrastructure(node); node ||= infra.defaultNode || infra.nodes[0] || ''; storage = infra.root.includes(storage) ? storage : infra.root[0] || ''; templateStorage = infra.templates.includes(templateStorage) ? templateStorage : infra.templates[0] || ''; bridge = infra.bridges.includes(bridge) ? bridge : infra.bridges[0] || ''; error = ''; }
    catch(e: any) { error = e.message; }
  }
  onMount(async () => { await discover(); try { const instances = await apiGet('/databases'); apps = (await Promise.all(instances.filter((i: any) => i.status === 'ready').map((i: any) => apiGet('/databases/' + i.id + '/apps')))).flat().filter((a: any) => a.status === 'ready'); } catch { /* Optional database attachment */ } });
  onDestroy(() => cancel?.());
  async function deploy() {
    error = ''; busy = true; events = []; cancel?.();
    try {
      const env_vars: Record<string,string> = {};
      for (const line of env.split('\n').filter(l => l.trim())) { const i = line.indexOf('='); if (i <= 0) throw new Error('Use KEY=value for each environment variable'); env_vars[line.slice(0,i)] = line.slice(i+1); }
      const result = await apiPost('/ansible/deploy',{image,hostname,vmid,node,cores:cpu,memory,disk_size:String(disk)+'G',template_storage:templateStorage,storage_pool:storage,net_bridge:bridge,ip_address:ip,gateway:gateway || null,app_port:port || null,expose_secureweb:expose,secureweb_domain:expose ? domain : null,env_vars,mountpoints:volumes.map(v => ({...v,storage:v.storage || storage})),database_id:database || null,recipe:recipe?.id ? {id:recipe.id,version:recipe.version} : null,health:recipe?.health || {port:port || null}});
      task = result.task_id; status = 'queued';
      cancel = watchJob(task,(ev,state) => { events = ev; status = state; if (['succeeded','failed','interrupted','cancelled'].includes(state)) busy = false; });
    } catch(e: any) { error = e.message; busy = false; }
  }
</script>
<div class="platform"><h1>Deploy a container</h1><p class="intro">Convert a Linux amd64 OCI image into an unprivileged LXC. Progress reports the actual Proxmox result.</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}
<form on:submit|preventDefault={deploy}>
<div class="panel"><h2>Image and resources</h2><div class="fields">
<label>OCI image<input required bind:value={image} placeholder="nginx:1.28-alpine" /></label><label>Hostname<input required bind:value={hostname} pattern="[a-zA-Z0-9][a-zA-Z0-9.-]*" /></label>
<label>Node<select required bind:value={node} on:change={discover}>{#each infra.nodes as n}<option>{n}</option>{/each}</select></label>
<label>VMID (0 allocates automatically)<input type="number" min="0" max="999999999" bind:value={vmid} required /></label>
<label>CPU cores<input type="number" min="1" max="128" bind:value={cpu} required /></label><label>Memory (MiB)<input type="number" min="64" bind:value={memory} required /></label><label>Root disk (GiB)<input type="number" min="1" bind:value={disk} required /></label>
<label>Root disk pool<select bind:value={storage} required>{#each infra.root as s}<option>{s}</option>{/each}</select></label>
<label>Template pool<select bind:value={templateStorage} required>{#each infra.templates as s}<option>{s}</option>{/each}</select></label></div><button type="button" class="secondary" on:click={discover}>Refresh infrastructure</button></div>
<div class="panel"><h2>Network and service</h2><div class="fields"><label>Bridge<select bind:value={bridge} required>{#each infra.bridges as b}<option>{b}</option>{/each}</select></label><label>IP configuration<input bind:value={ip} placeholder="dhcp or 192.168.1.20/24" required /></label><label>Gateway (static IP)<input bind:value={gateway} /></label><label>Readiness TCP port (0 disables)<input type="number" min="0" max="65535" bind:value={port} /></label><label>Gateway ingress<select bind:value={expose}><option value={false}>Disabled</option><option value={true}>Configured SecureWeb gateway</option></select></label>{#if expose}<label>Public domain<input bind:value={domain} required /></label>{/if}</div></div>
<div class="panel"><h2>Persistent volumes</h2><p class="muted">Managed storage is included in Proxmox backups. Host paths are available through the API for advanced use.</p>{#each volumes as volume,idx}<div class="fields"><label>Mount path<input bind:value={volume.container} required placeholder="/data" /></label><label>Size (GiB)<input type="number" min="1" bind:value={volume.size_gb} required /></label><label>Storage pool<select bind:value={volume.storage}><option value="">Use root disk pool</option>{#each infra.root as s}<option>{s}</option>{/each}</select></label><button type="button" class="danger" on:click={() => volumes = volumes.filter((_,i) => i !== idx)}>Remove volume</button></div>{/each}<button type="button" class="secondary" on:click={() => volumes = [...volumes,{container:'',size_gb:8,storage:''}]}>Add volume</button></div>
<div class="panel"><h2>Application configuration</h2><label>Environment variables (one KEY=value per line)<textarea bind:value={env} spellcheck="false"></textarea></label><label>Hosted application database<select bind:value={database}><option value="">None</option>{#each apps as app}<option value={app.id}>{app.name} / {app.username}</option>{/each}</select></label>{#if database}<p class="muted">DATABASE_URL and the trusted database certificate are delivered to this container. Allow the container's network in the database access settings.</p>{/if}</div>
<button disabled={busy || !storage || !templateStorage || !bridge}>{busy ? 'Deployment in progress' : 'Deploy container'}</button></form>
{#if task}<div class="panel"><h2>Deployment: {status}</h2><code>{task}</code><pre>{events.map(e => '['+e.step+'] '+e.message).join('\n')}</pre><p class="muted">You can leave this screen; the job continues and appears in Operation history.</p></div>{/if}</div>
