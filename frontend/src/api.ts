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

type HistoryEnvelope = ApiEnvelope<BackupHistory[]> & {
  pagination: { page: number; per_page: number; has_next: boolean };
};

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

export type HistoryQuery = {
  page?: number;
  per_page?: number;
  config_id?: string;
  status?: string;
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

  logout() {
    return this.request<{ message: string }>('/auth/logout', { method: 'POST' });
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

  history(query: HistoryQuery = {}) {
    const search = new URLSearchParams();
    Object.entries(query).forEach(([key, value]) => {
      if (value !== undefined && value !== '') {
        search.set(key, String(value));
      }
    });
    const suffix = search.size > 0 ? `?${search.toString()}` : '';
    return this.requestEnvelope<HistoryEnvelope>(`/backup-history${suffix}`);
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
    const envelope = await this.requestEnvelope<ApiEnvelope<T>>(path, init);
    return envelope.data;
  }

  private async requestEnvelope<T>(path: string, init: RequestInit = {}) {
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
    return (await response.json()) as T;
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

const WEEKDAY_NAMES = ['日', '一', '二', '三', '四', '五', '六'];

function expandRange(field: string): string[] {
  const result: string[] = [];
  for (const part of field.split(',')) {
    const range = part.split('-');
    if (range.length === 2) {
      const start = parseInt(range[0], 10);
      const end = parseInt(range[1], 10);
      for (let i = start; i <= end; i++) result.push(String(i));
    } else {
      result.push(part);
    }
  }
  return result;
}

export function cronToHuman(expr: string | null | undefined): string {
  if (!expr) return '';
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 6) return expr;
  const [sec, min, hour, day, month, weekday] = parts;

  const secondOk = sec === '0' || sec === '*';
  const minAll = min === '*';
  const hourAll = hour === '*';
  const dayAll = day === '*';
  const monthAll = month === '*';
  const weekdayAll = weekday === '*';

  const minInterval = min.startsWith('*/') ? min.slice(2) : null;
  const hourInterval = hour.startsWith('*/') ? hour.slice(2) : null;

  const fmt = (n: string) => n.padStart(2, '0');
  const isPlainNumber = (value: string) => /^\d+$/.test(value);

  if (!secondOk) return expr;

  // Every N minutes
  if (minInterval && hourAll && dayAll && monthAll && weekdayAll) {
    return `每 ${minInterval} 分鐘`;
  }
  // Every N hours
  if (isPlainNumber(min) && hourInterval && dayAll && monthAll && weekdayAll) {
    return `每 ${hourInterval} 小時的 ${fmt(min)} 分`;
  }
  // Specific minute every hour
  if (isPlainNumber(min) && hourAll && dayAll && monthAll && weekdayAll) {
    return `每小時 ${fmt(min)} 分`;
  }
  // Daily: specific hour + minute
  if (isPlainNumber(min) && isPlainNumber(hour) && dayAll && monthAll && weekdayAll) {
    return `每天 ${fmt(hour)}:${fmt(min)}`;
  }
  // Weekly: specific weekday(s)
  if (isPlainNumber(min) && isPlainNumber(hour) && !weekdayAll && dayAll && monthAll) {
    const days = expandRange(weekday).map(d => WEEKDAY_NAMES[parseInt(d, 10)] ?? d);
    return `每週${days.join('、')} ${fmt(hour)}:${fmt(min)}`;
  }
  // Monthly: specific day of month
  if (isPlainNumber(min) && isPlainNumber(hour) && !dayAll && monthAll && weekdayAll) {
    return `每月 ${day} 日 ${fmt(hour)}:${fmt(min)}`;
  }
  // Yearly: specific month + day
  if (isPlainNumber(min) && isPlainNumber(hour) && !dayAll && !monthAll && weekdayAll) {
    return `每年 ${month} 月 ${day} 日 ${fmt(hour)}:${fmt(min)}`;
  }

  return expr;
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
