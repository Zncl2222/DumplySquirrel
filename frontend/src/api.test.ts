import { describe, expect, it, vi } from 'vitest';
import { ApiClient, cronToHuman, formatBytes } from './api';

function jsonResponse(body: unknown, init: ResponseInit = {}) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
    ...init,
  });
}

describe('ApiClient', () => {
  it('posts login credentials as JSON', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ data: { token: 'token-1', expires_at: 123, user: { id: '1', username: 'admin', role: 'admin' } } }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(new ApiClient(null).login('admin', 'secret')).resolves.toMatchObject({ token: 'token-1' });

    expect(fetchMock).toHaveBeenCalledWith('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username: 'admin', password: 'secret' }),
      headers: { 'Content-Type': 'application/json' },
    });
  });

  it('adds auth headers and serializes history query params', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ data: [], pagination: { page: 2, per_page: 20, has_next: false } }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(new ApiClient('token-1').history({ page: 2, per_page: 20, config_id: 'cfg-1' })).resolves.toEqual({
      data: [],
      pagination: { page: 2, per_page: 20, has_next: false },
    });

    expect(fetchMock).toHaveBeenCalledWith('/api/backup-history?page=2&per_page=20&config_id=cfg-1', {
      headers: { Authorization: 'Bearer token-1' },
    });
  });

  it('uses backend error messages when requests fail', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ error: { message: 'unauthorized' } }, { status: 401 })));

    await expect(new ApiClient('bad-token').me()).rejects.toThrow('unauthorized');
  });

  it('submits downloads natively instead of buffering the backup as a Blob', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    let submission: Record<string, string | null> | undefined;
    const submit = vi
      .spyOn(HTMLFormElement.prototype, 'submit')
      .mockImplementation(function captureSubmission(this: HTMLFormElement) {
        submission = {
          action: this.getAttribute('action'),
          method: this.method,
          target: this.target,
          token: new FormData(this).get('token')?.toString() ?? null,
        };
      });

    await new ApiClient('stream-token').downloadHistory('history/one');

    expect(fetchMock).toHaveBeenCalledWith('/api/backup-history/history%2Fone/download', {
      method: 'HEAD',
      headers: { Authorization: 'Bearer stream-token' },
    });
    expect(submission).toEqual({
      action: '/api/backup-history/history%2Fone/download',
      method: 'post',
      target: 'dumply-backup-download',
      token: 'stream-token',
    });
    expect(document.querySelector('form')).toBeNull();
    document.querySelector('iframe[name="dumply-backup-download"]')?.remove();
    submit.mockRestore();
  });

  it('surfaces a failed download preflight without submitting a hidden form', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 404 })));
    const submit = vi.spyOn(HTMLFormElement.prototype, 'submit');

    await expect(new ApiClient('stream-token').downloadHistory('missing')).rejects.toThrow(
      'Request failed with 404',
    );
    expect(submit).not.toHaveBeenCalled();
  });
});

describe('cronToHuman', () => {
  it.each([
    ['0 */15 * * * *', '每 15 分鐘'],
    ['0 30 */2 * * *', '每 2 小時的 30 分'],
    ['0 5 2 * * *', '每天 02:05'],
    ['0 0 8 * * 1-3', '每週一、二、三 08:00'],
    ['0 0 9 1 * *', '每月 1 日 09:00'],
    ['0 0 10 2 6 *', '每年 6 月 2 日 10:00'],
  ])('formats %s', (expr, expected) => {
    expect(cronToHuman(expr)).toBe(expected);
  });

  it('returns the original expression for unsupported cron shapes', () => {
    expect(cronToHuman('bad cron')).toBe('bad cron');
    expect(cronToHuman('15 0 2 * * *')).toBe('15 0 2 * * *');
  });
});

describe('formatBytes', () => {
  it.each([
    [null, '0 B'],
    [0, '0 B'],
    [512, '512 B'],
    [1024, '1.0 KB'],
    [1536, '1.5 KB'],
    [1048576, '1.0 MB'],
  ])('formats %s bytes', (value, expected) => {
    expect(formatBytes(value)).toBe(expected);
  });
});
