import React, { FormEvent, useCallback, useEffect, useMemo, useRef, useState, useDeferredValue } from 'react';
import {
  ApiClient,
  BackupConfig,
  BackupEvent,
  BackupHistory,
  ConfigPayload,
  DashboardStats,
  RunningBackup,
  User,
  cronToHuman,
  formatBytes,
} from './api';
import { useLanguage } from './i18n';
import { LanguageSwitcher } from './components/LanguageSwitcher';

type Tab = 'dashboard' | 'configs' | 'history' | 'users';

type RefreshOptions = {
  historyConfigId?: string | null;
  historyPage?: number;
};

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
  email_to: string;
  email_cc: string;
  email_notify_on: 'never' | 'failure' | 'always';
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
  email_to: '',
  email_cc: '',
  email_notify_on: 'never',
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

function getWeekdayOptions(t: (key: string) => string) {
  return [
    { value: '0', label: t('weekdays.sunday') },
    { value: '1', label: t('weekdays.monday') },
    { value: '2', label: t('weekdays.tuesday') },
    { value: '3', label: t('weekdays.wednesday') },
    { value: '4', label: t('weekdays.thursday') },
    { value: '5', label: t('weekdays.friday') },
    { value: '6', label: t('weekdays.saturday') },
  ];
}

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

function scheduleSummary(schedule: string, t: (key: string) => string) {
  return schedule ? `${cronToHuman(schedule, t)} ${t('scheduleSummary.start')}` : t('scheduleSummary.manual');
}

function parseRecipients(value: string) {
  return value
    .split(/[\n,;]/)
    .map((item) => item.trim())
    .filter(Boolean)
    .filter((item, index, items) => items.indexOf(item) === index);
}

function formatRecipients(values: string[]) {
  return values.join(', ');
}

function notificationSummary(config: BackupConfig, t: (key: string, params?: Record<string, string | number>) => string) {
  const notifyOn = config.email_notify_on ?? 'never';
  if (notifyOn === 'never') return t('configs.notifications.summaryNever');
  const count = (config.email_to ?? []).length + (config.email_cc ?? []).length;
  if (notifyOn === 'failure') return t('configs.notifications.summaryFailure', { count });
  return t('configs.notifications.summaryAlways', { count });
}

export function App() {
  const { t } = useLanguage();
  const [token, setToken] = useState(() => localStorage.getItem('dumply_token'));
  const [api] = useState(() => new ApiClient(token));
  const [user, setUser] = useState<User | null>(null);
  const [tab, setTab] = useState<Tab>('dashboard');
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [configs, setConfigs] = useState<BackupConfig[]>([]);
  const [history, setHistory] = useState<BackupHistory[]>([]);
  const [runningBackups, setRunningBackups] = useState<RunningBackup[]>([]);
  const [focusedHistory, setFocusedHistory] = useState<BackupHistory | null>(null);
  const [historyConfigId, setHistoryConfigId] = useState<string | null>(null);
  const [historyPage, setHistoryPage] = useState(1);
  const [historyHasNext, setHistoryHasNext] = useState(false);
  const historyConfigIdRef = useRef(historyConfigId);
  const historyPageRef = useRef(historyPage);
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    historyConfigIdRef.current = historyConfigId;
  }, [historyConfigId]);

  useEffect(() => {
    historyPageRef.current = historyPage;
  }, [historyPage]);

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

  useEffect(() => {
    if (!token || !user) return;
    let cancelled = false;
    async function pollRunRoom() {
      try {
        const runs = await api.runningBackups();
        if (!cancelled) setRunningBackups(runs);
      } catch {
        // Keep the global shell quiet during background polling.
      }
    }
    void pollRunRoom();
    const timer = window.setInterval(pollRunRoom, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [api, token, user]);

  const refreshAll = useCallback(async (options: RefreshOptions = {}) => {
    const requestedHistoryConfigId = 'historyConfigId' in options ? options.historyConfigId : historyConfigIdRef.current;
    const requestedHistoryPage = options.historyPage ?? historyPageRef.current;
    setLoading(true);
    setError(null);
    try {
      const [me, dashboardStats, backupConfigs, backupHistory, activeRuns, userList] = await Promise.all([
        api.me(),
        api.stats(),
        api.configs(),
        api.history({ page: requestedHistoryPage, per_page: 20, config_id: requestedHistoryConfigId ?? undefined }),
        api.runningBackups(),
        api.users(),
      ]);
      setUser(me);
      setStats(dashboardStats);
      setConfigs(backupConfigs);
      setHistory(backupHistory.data);
      setRunningBackups(activeRuns);
      setHistoryPage(backupHistory.pagination.page);
      setHistoryHasNext(backupHistory.pagination.has_next);
      setUsers(userList);
    } catch (err) {
      const message = err instanceof Error ? err.message : t('error.loadFailed');
      setError(message);
      if (message.includes('unauthorized')) {
        setToken(null);
      }
    } finally {
      setLoading(false);
    }
  }, [api]);

  const logout = useCallback(async () => {
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
      setRunningBackups([]);
      setUsers([]);
      setLoading(false);
    }
  }, [api]);

  const loadHistoryPage = useCallback(async (page: number, configId: string | null = historyConfigIdRef.current) => {
    setLoading(true);
    setError(null);
    try {
      const response = await api.history({ page, per_page: 20, config_id: configId ?? undefined });
      setHistory(response.data);
      setHistoryPage(response.pagination.page);
      setHistoryHasNext(response.pagination.has_next);
    } catch (err) {
      setError(err instanceof Error ? err.message : t('error.loadHistoryFailed'));
    } finally {
      setLoading(false);
    }
  }, [api]);

  function openDashboard() {
    setHistoryConfigId(null);
    setTab('dashboard');
    void loadHistoryPage(1, null);
  }

  function openAllHistory() {
    setFocusedHistory(null);
    setHistoryConfigId(null);
    setTab('history');
    void loadHistoryPage(1, null);
  }

  function openConfigHistory(config: BackupConfig) {
    setFocusedHistory(null);
    setHistoryConfigId(config.id);
    setTab('history');
    void loadHistoryPage(1, config.id);
  }

  if (!token || !user) {
    return <LoginPage api={api} onLogin={setToken} error={error} setError={setError} />;
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand-lockup">
          <img
            className="brand-icon"
            src="/assets/dumply-squirrel-icon.png"
            alt=""
            aria-hidden="true"
            draggable="false"
          />
          <div>
            <div className="brand">DumplySquirrel</div>
            <p className="muted">{t('app.subtitle')}</p>
          </div>
        </div>
        <nav>
          <button className={tab === 'dashboard' ? 'active' : ''} onClick={openDashboard}>{t('nav.dashboard')}</button>
          <button className={tab === 'configs' ? 'active' : ''} onClick={() => setTab('configs')}>{t('nav.configs')}</button>
          <button className={tab === 'history' ? 'active' : ''} onClick={openAllHistory}>{t('nav.history')}</button>
          <button className={tab === 'users' ? 'active' : ''} onClick={() => setTab('users')}>{t('nav.users')}</button>
        </nav>
        <div className="sidebar-footer">
          <span>{user.username}</span>
          <button className="ghost" onClick={logout}>{t('nav.logout')}</button>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{tabTitle(tab, t)}</h1>
            <p className="muted">{loading ? t('topbar.syncing') : t('topbar.synced')}</p>
          </div>
          <div className="topbar-actions">
            <LanguageSwitcher />
            <button onClick={() => void refreshAll()}>{t('topbar.refresh')}</button>
          </div>
        </header>
        {error && <div className="alert">{error}</div>}
        {tab === 'dashboard' && <Dashboard stats={stats} configs={configs} history={history} />}
        {tab === 'configs' && <Configs api={api} configs={configs} runningBackups={runningBackups} refresh={refreshAll} setError={setError} onViewHistory={openConfigHistory} onBackupStarted={(row) => { setFocusedHistory(row); setHistoryConfigId(row.config_id); setTab('history'); }} />}
        {tab === 'history' && <History api={api} history={history} runningBackups={runningBackups} focusedHistory={focusedHistory} configs={configs} selectedConfigId={historyConfigId} page={historyPage} hasNext={historyHasNext} refresh={refreshAll} setError={setError} onPageChange={loadHistoryPage} onShowAll={openAllHistory} onFocusedHistoryConsumed={() => setFocusedHistory(null)} />}
        {tab === 'users' && <Users api={api} users={users} refresh={refreshAll} setError={setError} currentUser={user} />}
      </main>
    </div>
  );
}

function LoginPage({ api, onLogin, error, setError }: { api: ApiClient; onLogin: (token: string) => void; error: string | null; setError: (error: string | null) => void }) {
  const { t } = useLanguage();
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
      setError(err instanceof Error ? err.message : t('login.error'));
    } finally {
      setLoading(false);
    }
  }

  return (
    <main className="login-page">
      <section className="login-card">
        <div className="login-brand">
          <img className="brand-icon" src="/assets/dumply-squirrel-icon.png" alt="" aria-hidden="true" draggable="false" />
          <p className="eyebrow">{t('app.name')}</p>
        </div>
        <h1>{t('login.title')}</h1>
        <form onSubmit={submit}>
          <label>{t('login.username')}<input maxLength={100} value={username} onChange={(event) => setUsername(event.target.value)} /></label>
          <label>{t('login.password')}<input type="password" maxLength={72} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
          {error && <div className="alert">{error}</div>}
          <button disabled={loading}>{loading ? t('login.loading') : t('login.submit')}</button>
        </form>
      </section>
    </main>
  );
}

function Dashboard({ stats, configs, history }: { stats: DashboardStats | null; configs: BackupConfig[]; history: BackupHistory[] }) {
  const { t } = useLanguage();
  const latest = history.slice(0, 6);
  const totalConfigs = stats?.total_configs ?? configs.length;
  const protectedConfigs = stats?.protected_configs ?? configs.filter((config) => config.last_success_at).length;
  return (
    <section className="grid">
      <Metric title={t('dashboard.totalTasks')} value={totalConfigs} />
      <Metric title={t('dashboard.protectedTasks')} value={`${protectedConfigs}/${totalConfigs}`} />
      <Metric title={t('dashboard.runningNow')} value={stats?.running_count ?? history.filter((item) => item.status === 'running').length} />
      <Metric title={t('dashboard.totalBackups')} value={stats?.total_backups ?? history.length} />
      <Metric title={t('dashboard.success')} value={stats?.success_count ?? 0} />
      <Metric title={t('dashboard.failed')} value={stats?.failed_count ?? 0} />
      <Metric title={t('dashboard.storage')} value={formatBytes(stats?.storage_bytes)} />
      <Metric title={t('dashboard.pendingCleanup')} value={stats?.pending_file_deletions ?? 0} />
      <Metric title={t('dashboard.lastSuccess')} value={formatTimestamp(stats?.last_success_at ?? null)} />
      <div className="panel wide">
        <h2>{t('dashboard.recentBackups')}</h2>
        <HistoryTable history={latest} configs={configs} compact />
      </div>
    </section>
  );
}

const Metric = React.memo(function Metric({ title, value }: { title: string; value: string | number }) {
  return <div className="metric"><span>{title}</span><strong>{value}</strong></div>;
});

const runStages = [
  { key: 'queued', label: 'queue' },
  { key: 'connect', label: 'connect' },
  { key: 'probe', label: 'probe' },
  { key: 'client', label: 'client' },
  { key: 'dump', label: 'dump' },
  { key: 'seal', label: 'seal' },
  { key: 'retention', label: 'retain' },
  { key: 'done', label: 'done' },
];

export function RunRoomDetail({ history, configName, events, loading, cancellationRequested, onBack, onCancel }: { history: BackupHistory; configName: string; events: BackupEvent[]; loading: boolean; cancellationRequested: boolean; onBack: () => void; onCancel: () => void }) {
  const { t } = useLanguage();
  const currentStage = stageFromEvents(events, history.status);
  const isRunning = history.status === 'running';
  const startedAt = new Date(history.started_at).toLocaleString();

  return (
    <section className="run-detail-panel" aria-labelledby="run-detail-title">
      <nav className="run-breadcrumb" aria-label={t('history.detail.breadcrumb')}>
        <span>{t('nav.history')}</span>
        <span aria-hidden="true">/</span>
        <span>{configName}</span>
        <span aria-hidden="true">/</span>
        <span>{startedAt}</span>
      </nav>

      <header className="run-detail-heading">
        <div className="run-detail-title">
          <button className="run-back" onClick={onBack}>{t('history.detail.back')}</button>
          <p className="run-kicker">{t('history.detail.kicker')}</p>
          <h2 id="run-detail-title">{configName}</h2>
          <p>{isRunning ? t('history.detail.runningDescription') : t('history.detail.completedDescription')}</p>
        </div>
        <div className="run-detail-badges">
          <StatusBadge status={history.status} />
          {isRunning && <span className="run-polling">{t('history.detail.polling')}</span>}
          {isRunning && (
            <button className="danger run-cancel" disabled={cancellationRequested} onClick={onCancel}>
              {cancellationRequested ? t('history.detail.cancellationRequested') : t('history.detail.cancelRun')}
            </button>
          )}
        </div>
      </header>

      <div className="run-facts">
        <RunFact label={t('history.detail.status')} value={history.status} />
        <RunFact label={t('history.detail.stage')} value={stageLabel(currentStage)} />
        <RunFact label={isRunning ? t('history.detail.duration') : t('history.detail.completedDuration')} value={runDuration(history)} />
        <RunFact label={t('history.detail.trigger')} value={history.triggered_by} />
      </div>

      <main className="run-log-panel" aria-label={t('history.detail.eventsTitle')}>
        <div className="run-log-head">
          <div>
            <p className="run-kicker">{loading ? t('history.detail.eventsLoading') : t('history.detail.eventsTitle')}</p>
            <h2>{t('history.detail.eventsTitle')}</h2>
          </div>
        </div>
        <RunLog events={events} history={history} />
      </main>
    </section>
  );
}

function RunFact({ label, value }: { label: string; value: string }) {
  return <div className="run-fact"><span>{label}</span><strong>{value}</strong></div>;
}

function RunLog({ events, history }: { events: BackupEvent[]; history: BackupHistory | null }) {
  const { t } = useLanguage();
  if (!history) {
    return <div className="run-log-empty">{t('history.detail.noEvents')}</div>;
  }

  return (
    <div className="run-log-lines" aria-live="polite">
      {events.length > 0 ? events.map((event) => (
        <div key={event.id} className={`run-log-line level-${event.level}`}>
          <span className="run-log-time">{new Date(event.created_at).toLocaleTimeString()}</span>
          <span className="run-log-stage">{event.stage}</span>
          <span>{event.message}</span>
        </div>
      )) : <div className="run-log-empty">{t('history.detail.waitingEvents')}</div>}
      {history.error_message && <div className="run-log-error">{history.error_message}</div>}
    </div>
  );
}

function stageFromEvents(events: BackupEvent[], status?: string) {
  const last = events.at(-1)?.stage;
  if (last) return last;
  if (status === 'success') return 'done';
  if (status === 'failed' || status === 'timeout') return 'failed';
  if (status === 'cancelled') return 'cancelled';
  return 'queued';
}

function stageLabel(stage: string) {
  return runStages.find((item) => item.key === stage)?.label ?? stage;
}

function elapsedTime(startedAt: string) {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(startedAt).getTime()) / 1000));
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${String(minutes).padStart(2, '0')}:${String(rest).padStart(2, '0')}`;
}

function runDuration(history: BackupHistory) {
  if (!history.completed_at) return elapsedTime(history.started_at);
  const seconds = Math.max(0, Math.floor((new Date(history.completed_at).getTime() - new Date(history.started_at).getTime()) / 1000));
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${String(minutes).padStart(2, '0')}:${String(rest).padStart(2, '0')}`;
}

function formatTimestamp(value: string | null) {
  return value ? new Date(value).toLocaleString() : '—';
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

function Configs({ api, configs, runningBackups, refresh, setError, onViewHistory, onBackupStarted }: { api: ApiClient; configs: BackupConfig[]; runningBackups: RunningBackup[]; refresh: (options?: RefreshOptions) => Promise<void>; setError: (error: string | null) => void; onViewHistory: (config: BackupConfig) => void; onBackupStarted: (history: BackupHistory) => void }) {
  const { t } = useLanguage();
  const [form, setForm] = useState(emptyConfigForm);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [scheduleDialogOpen, setScheduleDialogOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<BackupConfig | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [search, setSearch] = useState('');
  const deferredSearch = useDeferredValue(search);
  const [typeFilter, setTypeFilter] = useState<'all' | 'postgres' | 'mysql'>('all');
  const panelFormRef = useRef<HTMLFormElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const runningConfigIds = useMemo(() => new Set(runningBackups.map((run) => run.history.config_id)), [runningBackups]);

  const filtered = useMemo(() => configs.filter((config) => {
    if (typeFilter !== 'all' && config.db_type !== typeFilter) return false;
    if (deferredSearch) {
      const q = deferredSearch.toLowerCase();
      return (
        config.name.toLowerCase().includes(q) ||
        config.db_url_masked.toLowerCase().includes(q)
      );
    }
    return true;
  }), [configs, deferredSearch, typeFilter]);

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
      cron_schedule: form.cron_schedule.trim() || null,
      retention_days: Number(form.retention_days),
      timeout_seconds: Number(form.timeout_seconds),
      max_backups: form.max_backups === '' ? null : Number(form.max_backups),
      is_enabled: form.is_enabled,
      email_to: parseRecipients(form.email_to),
      email_cc: parseRecipients(form.email_cc),
      email_notify_on: form.email_notify_on,
    };
    const dbUrl = form.db_url.trim();
    if (!editingId || dbUrl) {
      payload.db_url = dbUrl;
    }
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
      setError(err instanceof Error ? err.message : t('error.createConfigFailed'));
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
      email_to: formatRecipients(config.email_to ?? []),
      email_cc: formatRecipients(config.email_cc ?? []),
      email_notify_on: config.email_notify_on ?? 'never',
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
    } catch (err) {
      const message = err instanceof Error ? err.message : t('error.deleteConfigFailed');
      setError(message);
      throw err instanceof Error ? err : new Error(message);
    }

    // Once DELETE commits, close the confirmation independently from the best-effort refresh.
    // A later refresh error must not make the dialog claim that deletion itself failed.
    setDeleteTarget(null);
    try {
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t('error.loadFailed'));
    }
  }

  async function triggerBackup(config: BackupConfig) {
    setError(null);
    try {
      const row = await api.triggerConfig(config.id);
      onBackupStarted(row);
      await refresh({ historyConfigId: row.config_id, historyPage: 1 });
    } catch (err) {
      setError(err instanceof Error ? err.message : t('error.triggerBackupFailed'));
    }
  }

  const handleToggleConfig = useCallback(async (config: BackupConfig) => {
    try {
      await api.toggleConfig(config.id, !config.is_enabled);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t('error.toggleStatusFailed'));
    }
  }, [api, refresh, setError]);

  return (
    <section className="configs-page">
      <div className="config-header">
        <h2>{t('configs.title')}</h2>
        <p>{t('configs.description')}</p>
      </div>

      <div className="config-toolbar">
        <div className="config-search">
          <input placeholder={t('configs.searchPlaceholder')} value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
        <div className="config-filters">
          <button className={`config-type-btn type-all ${typeFilter === 'all' ? 'active' : ''}`} onClick={() => setTypeFilter('all')}>{t('configs.filterAll')}</button>
          <button className={`config-type-btn type-postgres ${typeFilter === 'postgres' ? 'active' : ''}`} onClick={() => setTypeFilter('postgres')}>PostgreSQL</button>
          <button className={`config-type-btn type-mysql ${typeFilter === 'mysql' ? 'active' : ''}`} onClick={() => setTypeFilter('mysql')}>MySQL</button>
        </div>
        <span className="config-count">{filtered.length} / {configs.length}</span>
        <button onClick={openCreate}>{t('configs.addDatabase')}</button>
      </div>

      {filtered.length > 0 ? (
        <div className="panel config-table-panel">
          <div className="table-wrap config-table-wrap">
            <table className="config-table">
              <thead>
                <tr>
                  <th>{t('configs.columns.name')}</th>
                  <th>{t('configs.columns.type')}</th>
                  <th>{t('configs.columns.version')}</th>
                  <th>{t('configs.columns.schedule')}</th>
                  <th>{t('configs.columns.retention')}</th>
                  <th>{t('configs.columns.lastRun')}</th>
                  <th>{t('configs.columns.status')}</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((config) => (
                  <tr key={config.id} className={`row-${config.db_type} ${runningConfigIds.has(config.id) ? 'row-running' : ''}`}>
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
                      {scheduleSummary(config.cron_schedule ?? '', t)}
                      {config.cron_schedule && <code title={config.cron_schedule}>{config.cron_schedule}</code>}
                      <span className="notification-summary">{notificationSummary(config, t)}</span>
                    </td>
                    <td className="config-retention">{config.retention_days} {t('units.days')}</td>
                    <td>
                      {config.last_run_status ? (
                        <div className="config-lastbackup">
                          <StatusBadge status={config.last_run_status} />
                          <time dateTime={config.last_run_at ?? undefined}>{formatTimestamp(config.last_run_at)}</time>
                          {config.last_success_at && (
                            <small>{t('configs.lastRun.lastSuccess', { time: formatTimestamp(config.last_success_at) })}</small>
                          )}
                        </div>
                      ) : (
                        <span className="muted config-lastbackup-never">{t('configs.lastRun.never')}</span>
                      )}
                    </td>
                    <td><StatusBadge status={config.is_enabled ? 'enabled' : 'disabled'} /></td>
                    <td>
                      <div className="config-actions">
                        <button className="ghost" onClick={() => onViewHistory(config)}>{t('configs.actions.history')}</button>
                        <button className={`ghost backup-trigger ${runningConfigIds.has(config.id) ? 'running' : ''}`} disabled={runningConfigIds.has(config.id)} onClick={() => void triggerBackup(config)}>{runningConfigIds.has(config.id) ? t('configs.actions.backupRunning') : t('configs.actions.backup')}</button>
                        <button className="ghost" onClick={() => void handleToggleConfig(config)}>{config.is_enabled ? t('configs.actions.disable') : t('configs.actions.enable')}</button>
                        <button className="ghost" onClick={() => openEdit(config)}>{t('configs.actions.edit')}</button>
                        <button className="danger" onClick={() => setDeleteTarget(config)}>{t('configs.actions.delete')}</button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : (
        <div className="config-empty">
          <h3>{search || typeFilter !== 'all' ? t('configs.emptySearch.title') : t('configs.empty.title')}</h3>
          <p>{search || typeFilter !== 'all' ? t('configs.emptySearch.description') : t('configs.empty.description')}</p>
          {!search && typeFilter === 'all' && <button onClick={openCreate}>{t('configs.addDatabase')}</button>}
        </div>
      )}

      {showPanel && (
        <>
          <div className="config-overlay" onClick={closePanel} />
          <div className="config-panel" ref={panelRef} role="dialog" aria-label={editingId ? t('configs.form.editTitle') : t('configs.form.createTitle')}>
            <div className="config-panel-header">
              <h2>{editingId ? t('configs.form.editTitle') : t('configs.form.createTitle')}</h2>
              <button className="config-panel-close" onClick={closePanel} aria-label="Close">✕</button>
            </div>
            <form ref={panelFormRef} className="config-panel-body form" onSubmit={submit}>
              {editingId && <p className="panel-note">{t('configs.form.editNote')}</p>}
              <label>{t('configs.form.name')}<input required value={form.name} onChange={(e) => setForm((prev) => ({ ...prev, name: e.target.value }))} /></label>
              <label>{t('configs.form.dbType')}<select value={form.db_type} onChange={(e) => setForm((prev) => ({ ...prev, db_type: e.target.value as 'postgres' | 'mysql', db_version: '' }))}><option value="postgres">PostgreSQL</option><option value="mysql">MySQL</option></select></label>
              <label>{t('configs.form.dbVersion')}<select value={form.db_version} onChange={(e) => setForm((prev) => ({ ...prev, db_version: e.target.value }))}>{dbVersionOptions(form.db_type).map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
              {form.db_type === 'mysql' && <p className="muted">{t('mysql.note')}</p>}
              <label>{t('configs.form.dbUrl')}<input required={!editingId} placeholder={editingId ? t('configs.form.dbUrlEditPlaceholder') : t('configs.form.dbUrlPlaceholder')} value={form.db_url} onChange={(e) => setForm((prev) => ({ ...prev, db_url: e.target.value }))} /></label>
              <div className="schedule-summary">
                <span>{t('configs.form.schedule')}</span>
                <strong>{scheduleSummary(form.cron_schedule, t)}</strong>
                <p className="muted">{form.cron_schedule ? t('configs.form.scheduleDescription') : t('configs.form.scheduleManual')}</p>
                <button type="button" onClick={() => setScheduleDialogOpen(true)}>{t('configs.form.setSchedule')}</button>
              </div>
              <div className="form-row">
                <label>{t('configs.form.retentionDays')}<input type="number" min="1" value={form.retention_days} onChange={(e) => setForm((prev) => ({ ...prev, retention_days: Number(e.target.value) }))} /></label>
                <label>{t('configs.form.timeoutSeconds')}<input type="number" min="1" value={form.timeout_seconds} onChange={(e) => setForm((prev) => ({ ...prev, timeout_seconds: Number(e.target.value) }))} /></label>
              </div>
              <label>{t('configs.form.maxBackups')}<input type="number" min="1" value={form.max_backups} onChange={(e) => setForm((prev) => ({ ...prev, max_backups: e.target.value }))} /></label>
              <label className="check"><input type="checkbox" checked={form.is_enabled} onChange={(e) => setForm((prev) => ({ ...prev, is_enabled: e.target.checked }))} />{t('configs.form.enableSchedule')}</label>
              <fieldset className="notification-fields">
                <legend>{t('configs.notifications.title')}</legend>
                <label>{t('configs.notifications.notifyOn')}<select value={form.email_notify_on} onChange={(e) => setForm((prev) => ({ ...prev, email_notify_on: e.target.value as ConfigFormState['email_notify_on'] }))}><option value="never">{t('configs.notifications.never')}</option><option value="failure">{t('configs.notifications.failure')}</option><option value="always">{t('configs.notifications.always')}</option></select></label>
                <label>{t('configs.notifications.to')}<textarea rows={3} required={form.email_notify_on !== 'never'} placeholder={t('configs.notifications.toPlaceholder')} value={form.email_to} onChange={(e) => setForm((prev) => ({ ...prev, email_to: e.target.value }))} /></label>
                <label>{t('configs.notifications.cc')}<textarea rows={2} placeholder={t('configs.notifications.ccPlaceholder')} value={form.email_cc} onChange={(e) => setForm((prev) => ({ ...prev, email_cc: e.target.value }))} /></label>
                <p className="muted">{t('configs.notifications.description')}</p>
              </fieldset>
            </form>
            <div className="config-panel-footer">
              <button className="ghost" onClick={closePanel}>{t('configs.form.cancel')}</button>
              <button onClick={() => panelFormRef.current?.requestSubmit()}>{editingId ? t('configs.form.update') : t('configs.form.create')}</button>
            </div>
          </div>
        </>
      )}

      <ScheduleModal
        open={scheduleDialogOpen}
        value={form.cron_schedule}
        onApply={(schedule) => {
          setForm((prev) => ({ ...prev, cron_schedule: schedule }));
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
  const { t } = useLanguage();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [draft, setDraft] = useState(() => scheduleDraftFromCron(value));
  const cron = cronFromScheduleDraft(draft);
  const translated = cron ? cronToHuman(cron, t) : t('scheduleSummary.manual');

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
            <h2 id="schedule-dialog-title">{t('configs.schedule.title')}</h2>
            <p className="muted">{t('configs.schedule.description')}</p>
          </div>
          <button type="button" className="ghost" onClick={onClose}>{t('configs.form.cancel')}</button>
        </div>

        <label>{t('configs.schedule.type')}<select value={draft.mode} onChange={(event) => updateMode(event.target.value as ScheduleMode)}>
          <option value="manual">{t('configs.schedule.typeManual')}</option>
          <option value="minute">{t('configs.schedule.typeMinute')}</option>
          <option value="hour">{t('configs.schedule.typeHour')}</option>
          <option value="daily">{t('configs.schedule.typeDaily')}</option>
          <option value="weekly">{t('configs.schedule.typeWeekly')}</option>
          <option value="monthly">{t('configs.schedule.typeMonthly')}</option>
          <option value="yearly">{t('configs.schedule.typeYearly')}</option>
          <option value="custom">{t('configs.schedule.typeCustom')}</option>
        </select></label>

        {draft.mode === 'minute' && <label>{t('configs.schedule.everyMinutes', { interval: draft.minuteInterval })}<select value={draft.minuteInterval} onChange={(event) => setDraft({ ...draft, minuteInterval: event.target.value })}>{intervalOptions.map((option) => <option key={option} value={option}>{option}</option>)}</select></label>}
        {draft.mode === 'hour' && <div className="form-row"><label>{t('configs.schedule.everyHours', { interval: draft.hourInterval })}<select value={draft.hourInterval} onChange={(event) => setDraft({ ...draft, hourInterval: event.target.value })}>{hourIntervalOptions.map((option) => <option key={option} value={option}>{option}</option>)}</select></label><TimeFields time={draft.time} label={t('configs.schedule.startFromMinute')} onChange={(time) => setDraft({ ...draft, time })} minuteOnly /></div>}
        {['daily', 'weekly', 'monthly', 'yearly'].includes(draft.mode) && <TimeFields time={draft.time} label={t('configs.schedule.startTime')} onChange={(time) => setDraft({ ...draft, time })} />}
        {draft.mode === 'weekly' && <fieldset className="weekday-grid"><legend>{t('configs.schedule.weekdays')}</legend>{getWeekdayOptions(t).map((option) => <label key={option.value} className="check"><input type="checkbox" checked={draft.weekdays.includes(option.value)} onChange={() => toggleWeekday(option.value)} />{option.label}</label>)}</fieldset>}
        {draft.mode === 'monthly' && <label>{t('configs.schedule.monthDay')}<select value={draft.monthDay} onChange={(event) => setDraft({ ...draft, monthDay: event.target.value })}>{dayOptions.map((option) => <option key={option} value={option}>{option}</option>)}</select></label>}
        {draft.mode === 'yearly' && <div className="form-row"><label>{t('configs.schedule.yearMonth')}<select value={draft.month} onChange={(event) => setDraft({ ...draft, month: event.target.value })}>{monthOptions.map((option) => <option key={option} value={option}>{option}</option>)}</select></label><label>{t('configs.schedule.yearDay')}<select value={draft.monthDay} onChange={(event) => setDraft({ ...draft, monthDay: event.target.value })}>{dayOptions.map((option) => <option key={option} value={option}>{option}</option>)}</select></label></div>}
        {draft.mode === 'custom' && <label>{t('configs.schedule.customCron')}<input aria-describedby="schedule-preview" value={draft.cron} onChange={(event) => setDraft({ ...draft, cron: event.target.value })} /></label>}

        <div className="schedule-preview" id="schedule-preview" aria-live="polite">
          <span>{t('configs.schedule.preview')}</span>
          <strong>{cron ? `${translated} ${t('configs.schedule.previewStart')}` : translated}</strong>
          {cron && <code>{cron}</code>}
        </div>

        <div className="modal-actions">
          <button type="button" className="ghost" onClick={onClose}>{t('configs.form.cancel')}</button>
          <button type="submit">{t('configs.schedule.apply')}</button>
        </div>
      </form>
    </dialog>
  );
}

function TimeFields({ time, label, minuteOnly = false, onChange }: { time: string; label: string; minuteOnly?: boolean; onChange: (time: string) => void }) {
  const { t } = useLanguage();
  const { hour, minute } = timeParts(time);
  return (
    <fieldset className="time-fields">
      <legend>{label}</legend>
      {!minuteOnly && <label>{t('units.hours')}<select value={hour} onChange={(event) => onChange(updateTimePart(time, 'hour', event.target.value))}>{hourOptions.map((option) => <option key={option} value={option}>{option.padStart(2, '0')}</option>)}</select></label>}
      <label>{t('units.minutes')}<select value={minute} onChange={(event) => onChange(updateTimePart(time, 'minute', event.target.value))}>{minuteOptions.map((option) => <option key={option} value={option}>{option.padStart(2, '0')}</option>)}</select></label>
    </fieldset>
  );
}

export function DeleteConfigDialog({ config, onCancel, onConfirm }: { config: BackupConfig | null; onCancel: () => void; onConfirm: () => Promise<void> }) {
  const { t } = useLanguage();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    setSubmitting(false);
    setSubmitError(null);
    if (config && !dialog.open) dialog.showModal();
    if (!config && dialog.open) dialog.close();
  }, [config]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (submitting) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      await onConfirm();
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : t('error.deleteConfigFailed'));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <dialog
      className="schedule-dialog confirm-dialog"
      ref={dialogRef}
      onCancel={(event) => {
        if (submitting) {
          event.preventDefault();
          return;
        }
        onCancel();
      }}
      onClose={onCancel}
      aria-labelledby="delete-config-title"
      aria-describedby="delete-config-description"
    >
      <form className="schedule-form" onSubmit={submit}>
        <div>
          <h2 id="delete-config-title">{t('configs.delete.title')}</h2>
          <p id="delete-config-description" className="muted">{t('configs.delete.description', { name: config?.name ?? '' })}</p>
          {submitError && <p className="alert" role="alert">{submitError}</p>}
        </div>
        <div className="modal-actions">
          <button type="button" className="ghost" disabled={submitting} onClick={onCancel}>{t('configs.delete.cancel')}</button>
          <button type="submit" className="danger" disabled={submitting}>
            {submitting ? t('configs.delete.deleting') : t('configs.delete.confirm')}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function History({ api, history, runningBackups, focusedHistory, configs, selectedConfigId, page, hasNext, refresh, setError, onPageChange, onShowAll, onFocusedHistoryConsumed }: { api: ApiClient; history: BackupHistory[]; runningBackups: RunningBackup[]; focusedHistory: BackupHistory | null; configs: BackupConfig[]; selectedConfigId: string | null; page: number; hasNext: boolean; refresh: () => Promise<void>; setError: (error: string | null) => void; onPageChange: (page: number) => Promise<void>; onShowAll: () => void; onFocusedHistoryConsumed: () => void }) {
  const { t } = useLanguage();
  const [selectedHistory, setSelectedHistory] = useState<BackupHistory | null>(null);
  const [selectedEvents, setSelectedEvents] = useState<BackupEvent[]>([]);
  const [eventsLoading, setEventsLoading] = useState(false);
  const [cancellationRequestedFor, setCancellationRequestedFor] = useState<string | null>(null);
  const names = useMemo(() => new Map(configs.map((config) => [config.id, config.name])), [configs]);
  const selectedConfig = useMemo(() => configs.find((config) => config.id === selectedConfigId) ?? null, [configs, selectedConfigId]);
  const selectedConfigName = selectedConfig?.name ?? selectedConfigId?.slice(0, 8) ?? null;
  const mergedHistory = useMemo(() => {
    const runningRows = runningBackups
      .filter((run) => !selectedConfigId || run.history.config_id === selectedConfigId)
      .map((run) => run.history);
    const runningIds = new Set(runningRows.map((row) => row.id));
    const historyRows = selectedConfigId
      ? history.filter((row) => row.config_id === selectedConfigId)
      : history;
    return [...runningRows, ...historyRows.filter((row) => !runningIds.has(row.id))];
  }, [history, runningBackups, selectedConfigId]);

  useEffect(() => {
    if (!focusedHistory) return;
    inspect(focusedHistory);
    onFocusedHistoryConsumed();
  }, [focusedHistory, onFocusedHistoryConsumed]);

  useEffect(() => {
    if (!selectedHistory) return;
    const historyId = selectedHistory.id;
    const isRunning = selectedHistory.status === 'running';
    let cancelled = false;
    async function loadEvents() {
      setEventsLoading(true);
      try {
        const [events, latestHistory] = await Promise.all([
          api.backupEvents(historyId),
          isRunning ? api.historyItem(historyId) : Promise.resolve(null),
        ]);
        if (!cancelled) {
          setSelectedEvents(events);
          if (latestHistory && latestHistory.status !== 'running') setSelectedHistory(latestHistory);
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : t('error.loadEventsFailed'));
      } finally {
        if (!cancelled) setEventsLoading(false);
      }
    }
    void loadEvents();
    if (!isRunning) {
      return () => {
        cancelled = true;
      };
    }
    const timer = window.setInterval(loadEvents, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [api, selectedHistory, setError]);

  function inspect(row: BackupHistory) {
    const activeRun = runningBackups.find((run) => run.history.id === row.id);
    setSelectedHistory(activeRun?.history ?? row);
    setSelectedEvents(activeRun?.events ?? []);
  }

  function closeDetail() {
    setSelectedHistory(null);
    setSelectedEvents([]);
  }

  async function cancelSelectedRun() {
    if (!selectedHistory || selectedHistory.status !== 'running') return;
    if (!window.confirm(t('history.detail.cancelConfirm'))) return;
    const historyId = selectedHistory.id;
    setError(null);
    setCancellationRequestedFor(historyId);
    try {
      await api.cancelBackup(historyId);
      const events = await api.backupEvents(historyId);
      setSelectedEvents(events);
    } catch (err) {
      setCancellationRequestedFor(null);
      setError(err instanceof Error ? err.message : t('error.cancelBackupFailed'));
    }
  }

  if (selectedHistory) {
    const activeRun = runningBackups.find((run) => run.history.id === selectedHistory.id);
    const currentHistory = activeRun?.history ?? selectedHistory;
    return (
      <RunRoomDetail
        history={currentHistory}
        configName={names.get(currentHistory.config_id) ?? currentHistory.config_id.slice(0, 8)}
        events={activeRun?.events ?? selectedEvents}
        loading={eventsLoading}
        cancellationRequested={cancellationRequestedFor === currentHistory.id}
        onBack={closeDetail}
        onCancel={() => void cancelSelectedRun()}
      />
    );
  }

  return (
    <section className="panel history-panel">
      <div className="panel-heading">
        <div>
          <h2>{t('history.title')}</h2>
          <p className="muted">
            {selectedConfigName
              ? t('history.descriptionFiltered', { name: selectedConfigName })
              : t('history.descriptionAll')}
          </p>
        </div>
        <div className="actions">
          {selectedConfigId && <button className="ghost" onClick={onShowAll}>{t('history.viewAll')}</button>}
          <button onClick={() => void refresh()}>{t('topbar.refresh')}</button>
        </div>
      </div>
      <HistoryTable history={mergedHistory} configs={configs} onInspect={inspect} onDownload={(id) => api.downloadHistory(id).catch((err) => setError(err.message))} />
      <div className="pagination">
        <button className="ghost" disabled={page <= 1} onClick={() => void onPageChange(page - 1)}>{t('history.pagination.prev')}</button>
        <span>{t('history.pagination.page', { page })}</span>
        <button className="ghost" disabled={!hasNext} onClick={() => void onPageChange(page + 1)}>{t('history.pagination.next')}</button>
      </div>
    </section>
  );
}

const HistoryTable = React.memo(function HistoryTable({ history, configs, compact = false, onDownload, onInspect }: { history: BackupHistory[]; configs: BackupConfig[]; compact?: boolean; onDownload?: (id: string) => void; onInspect?: (row: BackupHistory) => void }) {
  const { t } = useLanguage();
  const names = useMemo(() => new Map(configs.map((config) => [config.id, config.name])), [configs]);
  return (
    <div className="table-wrap">
      <table>
        <thead><tr><th>{t('history.columns.task')}</th><th>{t('history.columns.status')}</th><th>{t('history.columns.trigger')}</th><th>{t('history.columns.size')}</th><th>{t('history.columns.started')}</th>{!compact && <th>{t('history.columns.actions')}</th>}</tr></thead>
        <tbody>
          {history.map((row) => (
            <tr key={row.id}>
              <td>{names.get(row.config_id) ?? row.config_id.slice(0, 8)}</td>
              <td><StatusBadge status={row.status} /></td>
              <td>{row.triggered_by}</td>
              <td>{formatBytes(row.file_size)}</td>
              <td>{new Date(row.started_at).toLocaleString()}</td>
              {!compact && <td><div className="history-actions"><button className="ghost" onClick={() => onInspect?.(row)}>{t('history.viewFlow')}</button>{row.is_downloadable && <button onClick={() => onDownload?.(row.id)}>{t('history.download')}</button>}</div></td>}
            </tr>
          ))}
        </tbody>
      </table>
      {history.length === 0 && <p className="muted empty">{t('history.empty')}</p>}
    </div>
  );
});

function Users({ api, users, refresh, setError, currentUser }: { api: ApiClient; users: User[]; refresh: () => Promise<void>; setError: (error: string | null) => void; currentUser: User }) {
  const { t } = useLanguage();
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
      setError(err instanceof Error ? err.message : t('error.createUserFailed'));
    }
  }

  const handleDeleteUser = useCallback(async (userId: string) => {
    try {
      await api.deleteUser(userId);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t('error.deleteUserFailed'));
    }
  }, [api, refresh, setError]);

  return (
    <section className="two-column narrow">
      <form className="panel form" onSubmit={submit}>
        <h2>{t('users.createAdmin')}</h2>
        <label>{t('users.username')}<input required maxLength={100} value={username} onChange={(event) => setUsername(event.target.value)} /></label>
        <label>{t('users.password')}<input required type="password" minLength={12} maxLength={72} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <button>{t('users.create')}</button>
      </form>
      <div className="panel">
        <h2>{t('users.title')}</h2>
        <table>
          <thead><tr><th>{t('users.columns.username')}</th><th>{t('users.columns.role')}</th><th>{t('users.columns.actions')}</th></tr></thead>
          <tbody>{users.map((item) => <tr key={item.id}><td>{item.username}</td><td>{item.role}</td><td>{item.id !== currentUser.id && <button className="danger" onClick={() => void handleDeleteUser(item.id)}>{t('users.delete')}</button>}</td></tr>)}</tbody>
        </table>
      </div>
    </section>
  );
}

const StatusBadge = React.memo(function StatusBadge({ status }: { status: string }) {
  return <span className={`badge ${status}`}>{status}</span>;
});

function tabTitle(tab: Tab, t: (key: string) => string) {
  return ({ dashboard: t('nav.dashboard'), configs: t('nav.configs'), history: t('nav.history'), users: t('nav.users') } satisfies Record<Tab, string>)[tab];
}
