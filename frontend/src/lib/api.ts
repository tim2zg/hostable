export function getToken(): string | null {
  return localStorage.getItem('hostable_token');
}
export function websocketUrl(path: string): string {
  const url = new URL('/api/ws/' + path, window.location.href);
  url.protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  url.searchParams.set('token', getToken() || '');
  return url.toString();
}
async function request(path: string, method = 'GET', body?: unknown) {
  const res = await fetch('/api' + path, {
    method,
    headers: {
      Authorization: 'Bearer ' + getToken(),
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
  });
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent('hostable-logout'));
    throw new Error('Unauthorized');
  }
  if (res.status === 204) return undefined;
  const text = await res.text();
  let data: any;
  try { data = JSON.parse(text); } catch { data = null; }
  if (!res.ok || data?.status === 'error') {
    throw new Error(data?.error || data?.message || text || 'Request failed (' + res.status + ')');
  }
  if (data === null) throw new Error('The server returned an invalid response');
  return data;
}
export const apiGet = (path: string) => request(path);
export const apiPost = (path: string, body: unknown) => request(path, 'POST', body);
export const apiPut = (path: string, body: unknown) => request(path, 'PUT', body);
export const apiDelete = (path: string) => request(path, 'DELETE');
