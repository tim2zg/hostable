export function getToken(): string | null {
  return localStorage.getItem('hostable_token');
}

export async function apiGet(path: string) {
  const res = await fetch(`/api${path}`, {
    headers: { 'Authorization': `Bearer ${getToken()}` }
  });
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent('hostable-logout'));
    throw new Error('Unauthorized');
  }
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

export async function apiPost(path: string, body: any) {
  const res = await fetch(`/api${path}`, {
    method: 'POST',
    headers: { 
      'Authorization': `Bearer ${getToken()}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(body)
  });
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent('hostable-logout'));
    throw new Error('Unauthorized');
  }
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

export async function apiDelete(path: string) {
  const res = await fetch(`/api${path}`, {
    method: 'DELETE',
    headers: { 'Authorization': `Bearer ${getToken()}` }
  });
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent('hostable-logout'));
    throw new Error('Unauthorized');
  }
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}
