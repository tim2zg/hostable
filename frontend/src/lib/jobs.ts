import { apiGet, websocketUrl } from './api';
export type TaskEvent = { sequence: number; timestamp: string; level: string; step: string; message: string };
export type Job = { id: string; kind: string; resource: string; status: string; created_at: number; events: TaskEvent[]; cancellable?: boolean; cancel_requested?: boolean };
export function watchJob(id: string, receive: (events: TaskEvent[], status: string) => void): () => void {
  let closed = false; let socket: WebSocket | null = null; let busy = false;
  const events = new Map<number, TaskEvent>();
  let state = 'queued';
  const publish = () => receive([...events.values()].sort((a,b) => a.sequence-b.sequence), state);
  function accept(event: TaskEvent) {
    events.set(event.sequence, event);
    if (event.step === 'COMPLETE') state = 'succeeded';
    if (event.step === 'FAILED') state = 'failed';
    if (event.step === 'INTERRUPTED') state = 'interrupted';
    if (event.step === 'CANCELLED') state = 'cancelled';
    publish();
  }
  async function poll() {
    if (closed || busy) return;
    busy = true;
    try {
      const job: Job = await apiGet('/jobs/' + encodeURIComponent(id));
      if (closed) return;
      job.events.forEach(e => events.set(e.sequence,e)); state = job.status; publish();
      if (['succeeded','failed','interrupted','cancelled'].includes(state)) stop();
      else if (!socket || socket.readyState === WebSocket.CLOSED) connect();
    } catch { /* Preserve progress during disconnects. The next poll replays the durable job. */ }
    finally { busy = false; }
  }
  function connect() {
    if (closed) return;
    socket = new WebSocket(websocketUrl('tasks/' + encodeURIComponent(id)));
    socket.onmessage = e => { try { accept(JSON.parse(e.data)); } catch { /* invalid event */ } };
    socket.onerror = () => socket?.close();
  }
  const timer = window.setInterval(poll, 4000);
  function stop() { closed = true; window.clearInterval(timer); socket?.close(); }
  connect(); void poll(); return stop;
}
export type Infrastructure = { nodes: string[]; defaultNode?: string; root: string[]; templates: string[]; bridges: string[] };
export async function infrastructure(node = ''): Promise<Infrastructure> {
  const query = node ? '?node=' + encodeURIComponent(node) : '';
  const [n,s,b] = await Promise.all([apiGet('/nodes'),apiGet('/node/storages' + query),apiGet('/node/bridges' + query)]);
  return { defaultNode: n.default_node, nodes: n.data.map((r: any) => r.node), root: s.data.filter((r: any) => r.content?.split(',').includes('rootdir') && r.active !== 0).map((r: any) => r.storage), templates: s.data.filter((r: any) => r.content?.split(',').includes('vztmpl') && r.active !== 0).map((r: any) => r.storage), bridges: b };
}
