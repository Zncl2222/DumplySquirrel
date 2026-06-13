const API_BASE = import.meta.env.VITE_API_BASE_URL ?? '/api';

export type User = {
  id: string;
  username: string;
  role: string;
  created_at?: string;
};

export type BackupConfig = {
  id: string;
  name: string;
  db_type: 'postgres' | 'mysql';
  db_version: string | null;
  db_url_masked: string;
  cron_schedule: string | null;
  is_enabled: boolean;
  retention_days: number;
  timeout_seconds: number;
  max_backups: number | null;
  created_at: string;
  updated_at: string;
};

export type BackupHistory = {
  id: string;
  config_id: string;
  status: string;
  file_name: string | null;
  file_size: number | null;
  error_message: string | null;
  started_at: string;
  completed_at: string | null;
  triggered_by: string;
};

export type DashboardStats = {
  total_configs: number;
  total_backups: number;
  success_count: number;
  failed_count: number;
  storage_bytes: number;
};

type ApiEnvelope<T> = { data: T };

export type ConfigPayload = {
  name: string;
  db_type: 'postgres' | 'mysql';
  db_version?: string | null;
  db_url: string;
  cron_schedule?: string | null;
  retention_days?: number;
  timeout_seconds?: number;
  max_backups?: number | null;
  is_enabled?: boolean;
};

export class ApiClient {
  constructor(private token: string | null) {}

  setToken(token: string | null) {
    this.token = token;
  }

  async login(username: string, password: string) {
    return this.request<{ token: string; expires_at: number; user: User }>('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    });
  }

  me() {
    return this.request<User>('/auth/me');
  }

  stats() {
    return this.request<DashboardStats>('/dashboard/stats');
  }

  configs() {
    return this.request<BackupConfig[]>('/backup-configs');
  }

  createConfig(payload: ConfigPayload) {
    return this.request<BackupConfig>('/backup-configs', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  }

  updateConfig(id: string, payload: ConfigPayload) {
    return this.request<BackupConfig>(`/backup-configs/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  }

  deleteConfig(id: string) {
    return this.request<{ deleted: boolean }>(`/backup-configs/${id}`, { method: 'DELETE' });
  }

  toggleConfig(id: string, is_enabled: boolean) {
    return this.request<BackupConfig>(`/backup-configs/${id}/toggle`, {
      method: 'PATCH',
      body: JSON.stringify({ is_enabled }),
    });
  }

  triggerConfig(id: string) {
    return this.request<BackupHistory>(`/backup-configs/${id}/trigger`, { method: 'POST' });
  }

  history() {
    return this.request<BackupHistory[]>('/backup-history');
  }

  users() {
    return this.request<User[]>('/users');
  }

  createUser(username: string, password: string) {
    return this.request<User>('/users', {
      method: 'POST',
      body: JSON.stringify({ username, password, role: 'admin' }),
    });
  }

  deleteUser(id: string) {
    return this.request<{ deleted: boolean }>(`/users/${id}`, { method: 'DELETE' });
  }

  async downloadHistory(id: string) {
    const response = await fetch(`${API_BASE}/backup-history/${id}/download`, {
      headers: this.headers(false),
    });
    if (!response.ok) {
      throw new Error(await responseError(response));
    }

    const blob = await response.blob();
    const disposition = response.headers.get('content-disposition') ?? '';
    const fileName = disposition.match(/filename="([^"]+)"/)?.[1] ?? 'backup.sql';
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = fileName;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
  }

  private async request<T>(path: string, init: RequestInit = {}) {
    const response = await fetch(`${API_BASE}${path}`, {
      ...init,
      headers: {
        ...this.headers(init.body !== undefined),
        ...init.headers,
      },
    });
    if (!response.ok) {
      throw new Error(await responseError(response));
    }
    const envelope = (await response.json()) as ApiEnvelope<T>;
    return envelope.data;
  }

  private headers(json: boolean) {
    const headers: Record<string, string> = {};
    if (json) {
      headers['Content-Type'] = 'application/json';
    }
    if (this.token) {
      headers.Authorization = `Bearer ${this.token}`;
    }
    return headers;
  }
}

async function responseError(response: Response) {
  try {
    const body = await response.json();
    return body.error?.message ?? `Request failed with ${response.status}`;
  } catch {
    return `Request failed with ${response.status}`;
  }
}

export function formatBytes(value: number | null | undefined) {
  if (!value) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
