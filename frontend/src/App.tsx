import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ApiClient,
  BackupConfig,
  BackupHistory,
  ConfigPayload,
  DashboardStats,
  User,
  cronToHuman,
  formatBytes,
} from './api';

type Tab = 'dashboard' | 'configs' | 'history' | 'users';

type ConfigFormState = {
  name: string;
  db_type: 'postgres' | 'mysql';
  db_version: string;
  db_url: string;
  cron_schedule: string;
  retention_days: number;
  timeout_seconds: number;
  max_backups: string;
  is_enabled: boolean;
};

const emptyConfigForm: ConfigFormState = {
  name: '',
  db_type: 'postgres',
  db_version: '',
  db_url: '',
  cron_schedule: '',
  retention_days: 30,
  timeout_seconds: 3600,
  max_backups: '',
  is_enabled: true,
};

type ScheduleMode = 'manual' | 'minute' | 'hour' | 'daily' | 'weekly' | 'monthly' | 'yearly' | 'custom';

type ScheduleDraft = {
  mode: ScheduleMode;
  time: string;
  minuteInterval: string;
  hourInterval: string;
  weekdays: string[];
  monthDay: string;
  month: string;
  cron: string;
};

const weekdayOptions = [
  { value: '0', label: '週日' },
  { value: '1', label: '週一' },
  { value: '2', label: '週二' },
  { value: '3', label: '週三' },
  { value: '4', label: '週四' },
  { value: '5', label: '週五' },
  { value: '6', label: '週六' },
];

const dayOptions = Array.from({ length: 31 }, (_, index) => String(index + 1));
const hourOptions = Array.from({ length: 24 }, (_, index) => String(index));
const minuteOptions = Array.from({ length: 60 }, (_, index) => String(index));
const monthOptions = Array.from({ length: 12 }, (_, index) => String(index + 1));
const intervalOptions = ['5', '10', '15', '30'];
const hourIntervalOptions = ['1', '2', '3', '4', '6', '8', '12'];

const defaultScheduleDraft: ScheduleDraft = {
  mode: 'daily',
  time: '02:00',
  minuteInterval: '30',
  hourInterval: '2',
  weekdays: ['1'],
  monthDay: '1',
  month: '1',
  cron: '0 0 2 * * *',
};

function expandCronField(field: string) {
  return field.split(',').flatMap((part) => {
    const [start, end] = part.split('-').map((value) => Number(value));
    if (Number.isInteger(start) && Number.isInteger(end) && start <= end) {
      return Array.from({ length: end - start + 1 }, (_, index) => String(start + index));
    }
    return part;
  });
}

function timeFromCron(hour: string, minute: string) {
  return `${hour.padStart(2, '0')}:${minute.padStart(2, '0')}`;
}

function timeParts(time: string) {
  const [hour = '0', minute = '0'] = time.split(':');
  return { hour: String(Number(hour)), minute: String(Number(minute)) };
}

function updateTimePart(time: string, part: 'hour' | 'minute', value: string) {
  const current = timeParts(time);
  const next = { ...current, [part]: value };
  return timeFromCron(next.hour, next.minute);
}

function scheduleDraftFromCron(schedule: string): ScheduleDraft {
  if (!schedule) return { ...defaultScheduleDraft, mode: 'manual', cron: '' };

  const parts = schedule.trim().split(/\s+/);
  if (parts.length !== 6) return { ...defaultScheduleDraft, mode: 'custom', cron: schedule };

  const [sec, min, hour, day, month, weekday] = parts;
  const base = { ...defaultScheduleDraft, cron: schedule };
  const isNumber = (value: string) => /^\d+$/.test(value);

  if (sec !== '0' && sec !== '*') return { ...base, mode: 'custom' };
  if (min.startsWith('*/') && hour === '*' && day === '*' && month === '*' && weekday === '*') {
    return { ...base, mode: 'minute', minuteInterval: min.slice(2) };
  }
  if (isNumber(min) && hour.startsWith('*/') && day === '*' && month === '*' && weekday === '*') {
    return { ...base, mode: 'hour', time: timeFromCron('0', min), hourInterval: hour.slice(2) };
  }
  if (isNumber(min) && isNumber(hour) && day === '*' && month === '*' && weekday === '*') {
    return { ...base, mode: 'daily', time: timeFromCron(hour, min) };
  }
  if (isNumber(min) && isNumber(hour) && day === '*' && month === '*' && weekday !== '*') {
    return { ...base, mode: 'weekly', time: timeFromCron(hour, min), weekdays: expandCronField(weekday) };
  }
  if (isNumber(min) && isNumber(hour) && day !== '*' && month === '*' && weekday === '*') {
    return { ...base, mode: 'monthly', time: timeFromCron(hour, min), monthDay: day };
  }
  if (isNumber(min) && isNumber(hour) && day !== '*' && month !== '*' && weekday === '*') {
    return { ...base, mode: 'yearly', time: timeFromCron(hour, min), monthDay: day, month };
  }

  return { ...base, mode: 'custom' };
}

function cronFromScheduleDraft(draft: ScheduleDraft) {
  const { hour, minute } = timeParts(draft.time);
  if (draft.mode === 'manual') return '';
  if (draft.mode === 'minute') return `0 */${draft.minuteInterval} * * * *`;
  if (draft.mode === 'hour') return `0 ${minute} */${draft.hourInterval} * * *`;
  if (draft.mode === 'daily') return `0 ${minute} ${hour} * * *`;
  if (draft.mode === 'weekly') return `0 ${minute} ${hour} * * ${draft.weekdays.join(',') || '1'}`;
  if (draft.mode === 'monthly') return `0 ${minute} ${hour} ${draft.monthDay} * *`;
  if (draft.mode === 'yearly') return `0 ${minute} ${hour} ${draft.monthDay} ${draft.month} *`;
  return draft.cron.trim();
}

function scheduleSummary(schedule: string) {
  return schedule ? `${cronToHuman(schedule)} 開始` : '手動（不自動備份）';
}

export function App() {
  const [token, setToken] = useState(() => localStorage.getItem('dumply_token'));
  const [api] = useState(() => new ApiClient(token));
  const [user, setUser] = useState<User | null>(null);
  const [tab, setTab] = useState<Tab>('dashboard');
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [configs, setConfigs] = useState<BackupConfig[]>([]);
  const [history, setHistory] = useState<BackupHistory[]>([]);
  const [historyPage, setHistoryPage] = useState(1);
  const [historyHasNext, setHistoryHasNext] = useState(false);
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    api.setToken(token);
    if (token) {
      localStorage.setItem('dumply_token', token);
      void refreshAll();
    } else {
      localStorage.removeItem('dumply_token');
      setUser(null);
    }
  }, [api, token]);

  async function refreshAll() {
    setLoading(true);
    setError(null);
    try {
      const [me, dashboardStats, backupConfigs, backupHistory, userList] = await Promise.all([
        api.me(),
        api.stats(),
        api.configs(),
        api.history({ page: historyPage, per_page: 20 }),
        api.users(),
      ]);
      setUser(me);
      setStats(dashboardStats);
      setConfigs(backupConfigs);
      setHistory(backupHistory.data);
      setHistoryPage(backupHistory.pagination.page);
      setHistoryHasNext(backupHistory.pagination.has_next);
      setUsers(userList);
    } catch (err) {
      const message = err instanceof Error ? err.message : '載入失敗';
      setError(message);
      if (message.includes('unauthorized')) {
        setToken(null);
      }
    } finally {
      setLoading(false);
    }
  }

  async function logout() {
    setLoading(true);
    try {
      await api.logout();
    } catch {
      // Local logout still needs to complete if the revocation request fails.
    } finally {
      setToken(null);
      setStats(null);
      setConfigs([]);
      setHistory([]);
      setUsers([]);
      setLoading(false);
    }
  }

  async function loadHistoryPage(page: number) {
    setLoading(true);
    setError(null);
    try {
      const response = await api.history({ page, per_page: 20 });
      setHistory(response.data);
      setHistoryPage(response.pagination.page);
      setHistoryHasNext(response.pagination.has_next);
    } catch (err) {
      setError(err instanceof Error ? err.message : '載入歷史記錄失敗');
    } finally {
      setLoading(false);
    }
  }

  if (!token || !user) {
    return <LoginPage api={api} onLogin={setToken} error={error} setError={setError} />;
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <div>
          <div className="brand">DumplySquirrel</div>
          <p className="muted">備份管理系統</p>
        </div>
        <nav>
          <button className={tab === 'dashboard' ? 'active' : ''} onClick={() => setTab('dashboard')}>Dashboard</button>
          <button className={tab === 'configs' ? 'active' : ''} onClick={() => setTab('configs')}>備份設定</button>
          <button className={tab === 'history' ? 'active' : ''} onClick={() => setTab('history')}>歷史記錄</button>
          <button className={tab === 'users' ? 'active' : ''} onClick={() => setTab('users')}>使用者</button>
        </nav>
        <div className="sidebar-footer">
          <span>{user.username}</span>
          <button className="ghost" onClick={logout}>登出</button>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{tabTitle(tab)}</h1>
            <p className="muted">{loading ? '同步資料中...' : '系統狀態已同步'}</p>
          </div>
          <button onClick={refreshAll}>重新整理</button>
        </header>
        {error && <div className="alert">{error}</div>}
        {tab === 'dashboard' && <Dashboard stats={stats} configs={configs} history={history} />}
        {tab === 'configs' && <Configs api={api} configs={configs} refresh={refreshAll} setError={setError} />}
        {tab === 'history' && <History api={api} history={history} configs={configs} page={historyPage} hasNext={historyHasNext} refresh={refreshAll} setError={setError} onPageChange={loadHistoryPage} />}
        {tab === 'users' && <Users api={api} users={users} refresh={refreshAll} setError={setError} currentUser={user} />}
      </main>
    </div>
  );
}

function LoginPage({ api, onLogin, error, setError }: { api: ApiClient; onLogin: (token: string) => void; error: string | null; setError: (error: string | null) => void }) {
  const [username, setUsername] = useState('admin');
  const [password, setPassword] = useState('');
  const [loading, setLoading] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setLoading(true);
    setError(null);
    try {
      const response = await api.login(username, password);
      onLogin(response.token);
    } catch (err) {
      setError(err instanceof Error ? err.message : '登入失敗');
    } finally {
      setLoading(false);
    }
  }

  return (
    <main className="login-page">
      <section className="login-card">
        <p className="eyebrow">DumplySquirrel</p>
        <h1>登入備份管理台</h1>
        <form onSubmit={submit}>
          <label>帳號<input value={username} onChange={(event) => setUsername(event.target.value)} /></label>
          <label>密碼<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} /></label>
          {error && <div className="alert">{error}</div>}
          <button disabled={loading}>{loading ? '登入中...' : '登入'}</button>
        </form>
      </section>
    </main>
  );
}

function Dashboard({ stats, configs, history }: { stats: DashboardStats | null; configs: BackupConfig[]; history: BackupHistory[] }) {
  const latest = history.slice(0, 6);
  return (
    <section className="grid">
      <Metric title="任務數" value={stats?.total_configs ?? configs.length} />
      <Metric title="備份次數" value={stats?.total_backups ?? history.length} />
      <Metric title="成功" value={stats?.success_count ?? 0} />
      <Metric title="失敗" value={stats?.failed_count ?? 0} />
      <Metric title="儲存用量" value={formatBytes(stats?.storage_bytes)} />
      <div className="panel wide">
        <h2>最近備份</h2>
        <HistoryTable history={latest} configs={configs} compact />
      </div>
    </section>
  );
}

function Metric({ title, value }: { title: string; value: string | number }) {
  return <div className="metric"><span>{title}</span><strong>{value}</strong></div>;
}

function dbVersionOptions(dbType: 'postgres' | 'mysql') {
  if (dbType === 'postgres') {
    return [
      { value: '', label: 'Auto detect' },
      { value: '14', label: 'PostgreSQL 14' },
      { value: '15', label: 'PostgreSQL 15' },
      { value: '16', label: 'PostgreSQL 16' },
      { value: '17', label: 'PostgreSQL 17' },
      { value: '18', label: 'PostgreSQL 18' },
    ];
  }

  return [
    { value: '', label: 'Auto detect' },
    { value: '8.0', label: 'MySQL 8.0' },
    { value: '8.4', label: 'MySQL 8.4 LTS' },
    { value: '9.7', label: 'MySQL 9.7 LTS' },
  ];
}

function Configs({ api, configs, refresh, setError }: { api: ApiClient; configs: BackupConfig[]; refresh: () => Promise<void>; setError: (error: string | null) => void }) {
  const [form, setForm] = useState(emptyConfigForm);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [scheduleDialogOpen, setScheduleDialogOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<BackupConfig | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [search, setSearch] = useState('');
  const [typeFilter, setTypeFilter] = useState<'all' | 'postgres' | 'mysql'>('all');
  const panelFormRef = useRef<HTMLFormElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => configs.filter((config) => {
    if (typeFilter !== 'all' && config.db_type !== typeFilter) return false;
    if (search) {
      const q = search.toLowerCase();
      return (
        config.name.toLowerCase().includes(q) ||
        config.db_url_masked.toLowerCase().includes(q)
      );
    }
    return true;
  }), [configs, search, typeFilter]);

  useEffect(() => {
    if (!showPanel) return;
    if (!panelRef.current) return;
    const focusable = panelRef.current.querySelector<HTMLElement>('input, select, button, [tabindex]:not([tabindex="-1"])');
    focusable?.focus();
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        closePanel();
        return;
      }
      if (event.key !== 'Tab') return;
      const el = panelRef.current;
      if (!el) return;
      const focusables = el.querySelectorAll<HTMLElement>('input, select, button, [tabindex]:not([tabindex="-1"])');
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [showPanel]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    const payload: ConfigPayload = {
      name: form.name,
      db_type: form.db_type,
      db_version: form.db_version || null,
      db_url: form.db_url,
      cron_schedule: form.cron_schedule.trim() || null,
      retention_days: Number(form.retention_days),
      timeout_seconds: Number(form.timeout_seconds),
      max_backups: form.max_backups === '' ? null : Number(form.max_backups),
      is_enabled: form.is_enabled,
    };
    try {
      if (editingId) {
        await api.updateConfig(editingId, payload);
      } else {
        await api.createConfig(payload);
      }
      setEditingId(null);
      setForm(emptyConfigForm);
      setShowPanel(false);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : '儲存設定失敗');
    }
  }

  function openCreate() {
    setEditingId(null);
    setForm(emptyConfigForm);
    setShowPanel(true);
  }

  function openEdit(config: BackupConfig) {
    setEditingId(config.id);
    setForm({
      name: config.name,
      db_type: config.db_type,
      db_version: config.db_version ?? '',
      db_url: '',
      cron_schedule: config.cron_schedule ?? '',
      retention_days: config.retention_days,
      timeout_seconds: config.timeout_seconds,
      max_backups: config.max_backups?.toString() ?? '',
      is_enabled: config.is_enabled,
    });
    setShowPanel(true);
  }

  function closePanel() {
    setShowPanel(false);
    setEditingId(null);
    setForm(emptyConfigForm);
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    try {
      await api.deleteConfig(deleteTarget.id);
      setDeleteTarget(null);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : '刪除設定失敗');
    }
  }

  return (
    <section className="configs-page">
      <div className="config-header">
        <h2>備份設定</h2>
        <p>管理你的資料庫備份任務。每個設定對應一個資料庫連線與排程。</p>
      </div>

      <div className="config-toolbar">
        <div className="config-search">
          <input placeholder="搜尋名稱或連線位址..." value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
        <div className="config-filters">
          <button className={`config-type-btn type-all ${typeFilter === 'all' ? 'active' : ''}`} onClick={() => setTypeFilter('all')}>全部</button>
          <button className={`config-type-btn type-postgres ${typeFilter === 'postgres' ? 'active' : ''}`} onClick={() => setTypeFilter('postgres')}>PostgreSQL</button>
          <button className={`config-type-btn type-mysql ${typeFilter === 'mysql' ? 'active' : ''}`} onClick={() => setTypeFilter('mysql')}>MySQL</button>
        </div>
        <span className="config-count">{filtered.length} / {configs.length}</span>
        <button onClick={openCreate}>新增資料庫</button>
      </div>

      {filtered.length > 0 ? (
        <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
          <table className="config-table">
            <thead>
              <tr>
                <th>名稱</th>
                <th>類型</th>
                <th>版本</th>
                <th>排程</th>
                <th>保留</th>
                <th>狀態</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((config) => (
                <tr key={config.id} className={`row-${config.db_type}`}>
                  <td>
                    <div className="config-name">{config.name}</div>
                    <span className="config-url" title={config.db_url_masked}>{config.db_url_masked}</span>
                  </td>
                  <td>
                    <span className={`db-type-badge ${config.db_type}`}>
                      {config.db_type === 'postgres' ? 'PG' : 'MY'}
                    </span>
                  </td>
                  <td className="config-retention">{config.db_version ?? 'Auto'}</td>
                  <td className="config-schedule">
                    {scheduleSummary(config.cron_schedule ?? '')}
                    {config.cron_schedule && <code title={config.cron_schedule}>{config.cron_schedule}</code>}
                  </td>
                  <td className="config-retention">{config.retention_days} 天</td>
                  <td><StatusBadge status={config.is_enabled ? 'enabled' : 'disabled'} /></td>
                  <td>
                    <div className="config-actions">
                      <button className="ghost" onClick={() => api.triggerConfig(config.id).then(refresh).catch((err) => setError(err.message))}>備份</button>
                      <button className="ghost" onClick={() => api.toggleConfig(config.id, !config.is_enabled).then(refresh).catch((err) => setError(err.message))}>{config.is_enabled ? '停用' : '啟用'}</button>
                      <button className="ghost" onClick={() => openEdit(config)}>編輯</button>
                      <button className="danger" onClick={() => setDeleteTarget(config)}>刪除</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="config-empty">
          <h3>{search || typeFilter !== 'all' ? '找不到符合的設定' : '還沒有備份設定'}</h3>
          <p>{search || typeFilter !== 'all' ? '嘗試調整搜尋條件或篩選器。' : '新增你的第一個資料庫備份任務，設定排程後系統會自動執行。'}</p>
          {!search && typeFilter === 'all' && <button onClick={openCreate}>新增資料庫</button>}
        </div>
      )}

      {showPanel && (
        <>
          <div className="config-overlay" onClick={closePanel} />
          <div className="config-panel" ref={panelRef} role="dialog" aria-label={editingId ? '編輯備份設定' : '新增備份設定'}>
            <div className="config-panel-header">
              <h2>{editingId ? '編輯備份設定' : '新增備份設定'}</h2>
              <button className="config-panel-close" onClick={closePanel} aria-label="關閉">✕</button>
            </div>
            <form ref={panelFormRef} className="config-panel-body form" onSubmit={submit}>
              {editingId && <p className="panel-note">更新設定需要重新輸入完整 DB URL，API 不會回傳完整連線字串。</p>}
              <label>名稱<input required value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} /></label>
              <label>資料庫類型<select value={form.db_type} onChange={(e) => setForm({ ...form, db_type: e.target.value as 'postgres' | 'mysql', db_version: '' })}><option value="postgres">PostgreSQL</option><option value="mysql">MySQL</option></select></label>
              <label>Dump 版本<select value={form.db_version} onChange={(e) => setForm({ ...form, db_version: e.target.value })}>{dbVersionOptions(form.db_type).map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
              {form.db_type === 'mysql' && <p className="muted">MySQL 目前使用系統 mysqldump；版本選項只作為設定標示用途。</p>}
              <label>DB URL<input required placeholder="postgres://user:pass@host:5432/db" value={form.db_url} onChange={(e) => setForm({ ...form, db_url: e.target.value })} /></label>
              <div className="schedule-summary">
                <span>自動備份排程</span>
                <strong>{scheduleSummary(form.cron_schedule)}</strong>
                <p className="muted">{form.cron_schedule ? '已設定週期性自動備份。' : '不會自動執行，只能手動點「立即備份」。'}</p>
                <button type="button" onClick={() => setScheduleDialogOpen(true)}>設定排程</button>
              </div>
              <div className="form-row">
                <label>保留天數<input type="number" min="1" value={form.retention_days} onChange={(e) => setForm({ ...form, retention_days: Number(e.target.value) })} /></label>
                <label>Timeout 秒<input type="number" min="1" value={form.timeout_seconds} onChange={(e) => setForm({ ...form, timeout_seconds: Number(e.target.value) })} /></label>
              </div>
              <label>最多保留份數<input type="number" min="1" value={form.max_backups} onChange={(e) => setForm({ ...form, max_backups: e.target.value })} /></label>
              <label className="check"><input type="checkbox" checked={form.is_enabled} onChange={(e) => setForm({ ...form, is_enabled: e.target.checked })} />啟用排程</label>
            </form>
            <div className="config-panel-footer">
              <button className="ghost" onClick={closePanel}>取消</button>
              <button onClick={() => panelFormRef.current?.requestSubmit()}>{editingId ? '更新設定' : '建立設定'}</button>
            </div>
          </div>
        </>
      )}

      <ScheduleModal
        open={scheduleDialogOpen}
        value={form.cron_schedule}
        onApply={(schedule) => {
          setForm({ ...form, cron_schedule: schedule });
          setScheduleDialogOpen(false);
        }}
        onClose={() => setScheduleDialogOpen(false)}
      />
      <DeleteConfigDialog
        config={deleteTarget}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={confirmDelete}
      />
    </section>
  );
}

function ScheduleModal({ open, value, onApply, onClose }: { open: boolean; value: string; onApply: (schedule: string) => void; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [draft, setDraft] = useState(() => scheduleDraftFromCron(value));
  const cron = cronFromScheduleDraft(draft);
  const translated = cron ? cronToHuman(cron) : '手動（不自動備份）';

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  useEffect(() => {
    if (open) setDraft(scheduleDraftFromCron(value));
  }, [open, value]);

  function updateMode(mode: ScheduleMode) {
    setDraft((current) => ({ ...current, mode, cron: mode === 'custom' ? (cron || '0 0 2 * * *') : current.cron }));
  }

  function toggleWeekday(day: string) {
    setDraft((current) => {
      if (current.weekdays.includes(day)) {
        const weekdays = current.weekdays.filter((value) => value !== day);
        return { ...current, weekdays: weekdays.length > 0 ? weekdays : current.weekdays };
      }
      return { ...current, weekdays: [...current.weekdays, day].sort() };
    });
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onApply(cron);
  }

  return (
    <dialog className="schedule-dialog" ref={dialogRef} onClose={onClose} aria-labelledby="schedule-dialog-title">
      <form className="schedule-form" onSubmit={submit}>
        <div className="modal-heading">
          <div>
            <h2 id="schedule-dialog-title">設定自動備份排程</h2>
            <p className="muted">選擇週期與時間，或切到自訂 cron。下方會即時顯示實際執行時間。</p>
          </div>
          <button type="button" className="ghost" onClick={onClose}>關閉</button>
        </div>

        <label>排程類型<select value={draft.mode} onChange={(event) => updateMode(event.target.value as ScheduleMode)}>
          <option value="manual">手動（不自動備份）</option>
          <option value="minute">每幾分鐘</option>
          <option value="hour">每幾小時</option>
          <option value="daily">每天</option>
          <option value="weekly">每週</option>
          <option value="monthly">每月</option>
          <option value="yearly">每年</option>
          <option value="custom">手動輸入 cron</option>
        </select></label>

        {draft.mode === 'minute' && <label>每幾分鐘執行<select value={draft.minuteInterval} onChange={(event) => setDraft({ ...draft, minuteInterval: event.target.value })}>{intervalOptions.map((option) => <option key={option} value={option}>每 {option} 分鐘</option>)}</select></label>}
        {draft.mode === 'hour' && <div className="form-row"><label>每幾小時執行<select value={draft.hourInterval} onChange={(event) => setDraft({ ...draft, hourInterval: event.target.value })}>{hourIntervalOptions.map((option) => <option key={option} value={option}>每 {option} 小時</option>)}</select></label><TimeFields time={draft.time} label="從每小時幾分開始" onChange={(time) => setDraft({ ...draft, time })} minuteOnly /></div>}
        {['daily', 'weekly', 'monthly', 'yearly'].includes(draft.mode) && <TimeFields time={draft.time} label="開始時間" onChange={(time) => setDraft({ ...draft, time })} />}
        {draft.mode === 'weekly' && <fieldset className="weekday-grid"><legend>星期幾執行</legend>{weekdayOptions.map((option) => <label key={option.value} className="check"><input type="checkbox" checked={draft.weekdays.includes(option.value)} onChange={() => toggleWeekday(option.value)} />{option.label}</label>)}</fieldset>}
        {draft.mode === 'monthly' && <label>每月幾號執行<select value={draft.monthDay} onChange={(event) => setDraft({ ...draft, monthDay: event.target.value })}>{dayOptions.map((option) => <option key={option} value={option}>{option} 日</option>)}</select></label>}
        {draft.mode === 'yearly' && <div className="form-row"><label>每年幾月執行<select value={draft.month} onChange={(event) => setDraft({ ...draft, month: event.target.value })}>{monthOptions.map((option) => <option key={option} value={option}>{option} 月</option>)}</select></label><label>幾號執行<select value={draft.monthDay} onChange={(event) => setDraft({ ...draft, monthDay: event.target.value })}>{dayOptions.map((option) => <option key={option} value={option}>{option} 日</option>)}</select></label></div>}
        {draft.mode === 'custom' && <label>手動輸入 cron<input aria-describedby="schedule-preview" value={draft.cron} onChange={(event) => setDraft({ ...draft, cron: event.target.value })} /></label>}

        <div className="schedule-preview" id="schedule-preview" aria-live="polite">
          <span>排程預覽</span>
          <strong>{cron ? `${translated} 開始` : translated}</strong>
          {cron && <code>{cron}</code>}
        </div>

        <div className="modal-actions">
          <button type="button" className="ghost" onClick={onClose}>取消</button>
          <button type="submit">套用排程</button>
        </div>
      </form>
    </dialog>
  );
}

function TimeFields({ time, label, minuteOnly = false, onChange }: { time: string; label: string; minuteOnly?: boolean; onChange: (time: string) => void }) {
  const { hour, minute } = timeParts(time);
  return (
    <fieldset className="time-fields">
      <legend>{label}</legend>
      {!minuteOnly && <label>小時<select value={hour} onChange={(event) => onChange(updateTimePart(time, 'hour', event.target.value))}>{hourOptions.map((option) => <option key={option} value={option}>{option.padStart(2, '0')} 時</option>)}</select></label>}
      <label>分鐘<select value={minute} onChange={(event) => onChange(updateTimePart(time, 'minute', event.target.value))}>{minuteOptions.map((option) => <option key={option} value={option}>{option.padStart(2, '0')} 分</option>)}</select></label>
    </fieldset>
  );
}

function DeleteConfigDialog({ config, onCancel, onConfirm }: { config: BackupConfig | null; onCancel: () => void; onConfirm: () => Promise<void> }) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (config && !dialog.open) dialog.showModal();
    if (!config && dialog.open) dialog.close();
  }, [config]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await onConfirm();
  }

  return (
    <dialog className="schedule-dialog confirm-dialog" ref={dialogRef} onClose={onCancel} aria-labelledby="delete-config-title">
      <form className="schedule-form" onSubmit={submit}>
        <div>
          <h2 id="delete-config-title">確定要刪除備份設定？</h2>
          <p className="muted">這會刪除「{config?.name ?? ''}」的設定，之後不會再依照這個排程自動備份。</p>
        </div>
        <div className="modal-actions">
          <button type="button" className="ghost" onClick={onCancel}>取消</button>
          <button type="submit" className="danger">確認刪除</button>
        </div>
      </form>
    </dialog>
  );
}

function History({ api, history, configs, page, hasNext, refresh, setError, onPageChange }: { api: ApiClient; history: BackupHistory[]; configs: BackupConfig[]; page: number; hasNext: boolean; refresh: () => Promise<void>; setError: (error: string | null) => void; onPageChange: (page: number) => Promise<void> }) {
  return (
    <section className="panel">
      <div className="panel-heading"><h2>歷史記錄</h2><button onClick={refresh}>重新整理</button></div>
      <HistoryTable history={history} configs={configs} onDownload={(id) => api.downloadHistory(id).catch((err) => setError(err.message))} />
      <div className="pagination">
        <button className="ghost" disabled={page <= 1} onClick={() => void onPageChange(page - 1)}>上一頁</button>
        <span>第 {page} 頁</span>
        <button className="ghost" disabled={!hasNext} onClick={() => void onPageChange(page + 1)}>下一頁</button>
      </div>
    </section>
  );
}

function HistoryTable({ history, configs, compact = false, onDownload }: { history: BackupHistory[]; configs: BackupConfig[]; compact?: boolean; onDownload?: (id: string) => void }) {
  const names = new Map(configs.map((config) => [config.id, config.name]));
  return (
    <div className="table-wrap">
      <table>
        <thead><tr><th>任務</th><th>狀態</th><th>觸發</th><th>大小</th><th>開始</th>{!compact && <th>操作</th>}</tr></thead>
        <tbody>
          {history.map((row) => (
            <tr key={row.id}>
              <td>{names.get(row.config_id) ?? row.config_id.slice(0, 8)}</td>
              <td><StatusBadge status={row.status} /></td>
              <td>{row.triggered_by}</td>
              <td>{formatBytes(row.file_size)}</td>
              <td>{new Date(row.started_at).toLocaleString()}</td>
              {!compact && <td>{row.status === 'success' && <button onClick={() => onDownload?.(row.id)}>下載</button>}</td>}
            </tr>
          ))}
        </tbody>
      </table>
      {history.length === 0 && <p className="muted empty">尚無記錄</p>}
    </div>
  );
}

function Users({ api, users, refresh, setError, currentUser }: { api: ApiClient; users: User[]; refresh: () => Promise<void>; setError: (error: string | null) => void; currentUser: User }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      await api.createUser(username, password);
      setUsername('');
      setPassword('');
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : '建立使用者失敗');
    }
  }

  return (
    <section className="two-column narrow">
      <form className="panel form" onSubmit={submit}>
        <h2>新增管理員</h2>
        <label>帳號<input required value={username} onChange={(event) => setUsername(event.target.value)} /></label>
        <label>密碼<input required type="password" minLength={12} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <button>建立使用者</button>
      </form>
      <div className="panel">
        <h2>使用者</h2>
        <table>
          <thead><tr><th>帳號</th><th>角色</th><th>操作</th></tr></thead>
          <tbody>{users.map((item) => <tr key={item.id}><td>{item.username}</td><td>{item.role}</td><td>{item.id !== currentUser.id && <button className="danger" onClick={() => api.deleteUser(item.id).then(refresh).catch((err) => setError(err.message))}>刪除</button>}</td></tr>)}</tbody>
        </table>
      </div>
    </section>
  );
}

function StatusBadge({ status }: { status: string }) {
  return <span className={`badge ${status}`}>{status}</span>;
}

function tabTitle(tab: Tab) {
  return ({ dashboard: 'Dashboard', configs: '備份設定', history: '歷史記錄', users: '使用者管理' } satisfies Record<Tab, string>)[tab];
}
