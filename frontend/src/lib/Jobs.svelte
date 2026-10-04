<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { apiGet,apiPost } from './api';
  import type { Job } from './jobs';
  let jobs: Job[] = []; let selected: Job | null = null; let error = ''; let timer: ReturnType<typeof setInterval>;
  let inspection:any=null;let busy=false;
  async function act(action:string){if(!selected)return;busy=true;error='';try{if(action==='inspect')inspection=await apiPost('/jobs/'+selected.id+'/inspect',{});else if(action==='cancel')await apiPost('/jobs/'+selected.id+'/cancel',{});else await apiPost('/jobs/'+selected.id+'/recover',{action});await refresh();}catch(e:any){error=e.message;}finally{busy=false;}}
  async function refresh() { try { jobs = await apiGet('/jobs'); if (selected) selected = jobs.find(j => j.id === selected?.id) || selected; error = ''; } catch(e: any) { error = e.message; } }
  onMount(() => { void refresh(); timer = setInterval(refresh,4000); }); onDestroy(() => clearInterval(timer));
</script>
<div class="platform"><h1>Operation history</h1><p class="intro">Inspect live ownership and partial resources, resume supported deployments, or recover the previous container after an interrupted update. Cancellation stops at safe checkpoints.</p>
<button on:click={refresh}>Refresh</button>{#if error}<p class="error">{error}</p>{/if}
<div class="panel scroll"><table><thead><tr><th>Operation</th><th>Resource</th><th>Status</th><th>Created</th><th></th></tr></thead><tbody>{#each jobs as job}<tr><td>{job.kind}</td><td>{job.resource}</td><td>{job.status}</td><td>{new Date(job.created_at * 1000).toLocaleString()}</td><td><button class="secondary" on:click={() => selected = job}>Details</button></td></tr>{/each}</tbody></table>{#if !jobs.length}<p class="muted">No operations recorded.</p>{/if}</div>
{#if selected}<div class="panel"><h2>{selected.resource}: {selected.status}</h2><code>{selected.id}</code><pre>{selected.events.map(e => '[' + e.step + '] ' + e.message).join('\n')}</pre><button class="secondary" disabled={busy} on:click={()=>act('inspect')}>Inspect live resources</button>{#if selected.cancellable&&['queued','running'].includes(selected.status)}<button class="danger" disabled={busy||selected.cancel_requested} on:click={()=>act('cancel')}>{selected.cancel_requested?'Cancellation requested':'Request cancellation'}</button>{/if}{#if inspection?.job.id===selected.id}<pre>{JSON.stringify(inspection,null,2)}</pre>{#each inspection.actions as action}<button disabled={busy} on:click={()=>act(action)}>{action==='rollback'?'Recover previous container':action==='resume'?'Resume deployment':'Confirm completed update'}</button>{/each}{/if}</div>{/if}</div>
