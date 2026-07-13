const KEY = "quark_admin_token";
export function getToken(): string | null { return localStorage.getItem(KEY); }
export function setToken(t: string): void { localStorage.setItem(KEY, t); }
export function clearToken(): void { localStorage.removeItem(KEY); }
export function hasToken(): boolean { return getToken() !== null; }
