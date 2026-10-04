<script lang="ts">
  import {onMount, onDestroy} from 'svelte';
  import {apiGet, apiPost, apiPut} from './api';
  let status:any=null;
  let automatic=false;
  let interval=6;
  let busy=false;
  let error='';
  let notice='';
  let poll:ReturnType<typeof setInterval>|undefined;
  async function refresh(loadPolicy=false) {
    try {
      status=await apiGet('/manager/updates');
      if(loadPolicy){automatic=status.policy.automatic;interval=status.policy.check_interval_hours;}
      error='';
    } catch(e:any) {error=status?.maintenance?'The portal is restarting. It will reconnect automatically.':e.message;}
  }
  async function act(fn:()=>Promise<void>) {
    busy=true;error='';notice='';
    try{await fn();}catch(e:any){error=e.message;}finally{busy=false;}
  }
  async function check() {
    await act(async()=>{status=await apiPost('/manager/updates/check',{});});
  }
  async function save() {
    await act(async()=>{status=await apiPut('/manager/updates/policy',{automatic,check_interval_hours:interval});notice='Update policy saved';});
  }
  async function install() {
    const version=status.latest.version;
    await act(async()=>{
      notice='Preparing and verifying the update…';
      await apiPost('/manager/updates/install',{version});
      notice='Update scheduled. The portal will reconnect after its restart.';
      await refresh();
    });
  }
  onMount(()=>{void refresh(true);poll=setInterval(()=>void refresh(),10000);});
  onDestroy(()=>{if(poll)clearInterval(poll);});
</script>

<section class="platform" aria-labelledby="portal-updates-heading">
  <div class="panel">
    <h2 id="portal-updates-heading">Portal updates</h2>
    <p>Keep the Hostable manager and its dashboard up to date from GitHub releases.</p>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if notice}<p role="status">{notice}</p>{/if}
    {#if status}
      <p>Installed: <code>{status.current_version}</code></p>
      {#if status.latest}<p>Latest: <a href={status.latest.url} target="_blank" rel="noreferrer">{status.latest.version}</a></p>{/if}
      <p class="muted">Last checked: {status.checked_at?new Date(status.checked_at*1000).toLocaleString():'Not checked yet'}</p>
      {#if status.error}<p class="error">{status.error}</p>{/if}
      {#if !status.can_install}<p class="muted">{status.unavailable_reason}</p>{/if}
      {#if status.operation}
        <p>Update {status.operation.version}: <strong>{status.operation.phase}</strong></p>
        <p>{status.operation.message}</p>
        {#if status.operation.phase==='interrupted'}<p class="error">Inspect the retained update staging directory and service logs before recovery.</p>{/if}
      {/if}
      <button disabled={busy||status.maintenance} on:click={check}>Check for updates</button>
      <button disabled={busy||status.maintenance||!status.can_install||!status.update_available||status.operation?.phase==='interrupted'} on:click={install}>Install {status.update_available?status.latest.version:'update'}</button>
      <form on:submit|preventDefault={save}>
        <label for="manager-update-interval">Check interval (hours)</label>
        <input id="manager-update-interval" type="number" min="1" max="168" required bind:value={interval}/>
        <label><input type="checkbox" disabled={!status.can_install} bind:checked={automatic}/> Automatically install new stable releases</label>
        <p class="muted">Installation waits for active operations to finish. The manager restarts briefly; the previous binary and a metadata/configuration backup are retained. A failed readiness check restores the previous installation.</p>
        <button disabled={busy||status.maintenance}>Save update policy</button>
      </form>
    {/if}
  </div>
</section>
