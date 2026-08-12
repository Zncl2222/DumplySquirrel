import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { BackupConfig } from './api';
import { DeleteConfigDialog } from './App';
import { LanguageProvider } from './i18n';

const config: BackupConfig = {
  id: 'config-1',
  name: 'Production database',
  db_type: 'postgres',
  db_version: '16',
  db_url_masked: 'postgres://user:****@db/app',
  cron_schedule: null,
  is_enabled: true,
  retention_days: 30,
  timeout_seconds: 3600,
  max_backups: null,
  email_to: [],
  email_cc: [],
  email_notify_on: 'never',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  last_run_status: null,
  last_run_at: null,
  last_success_at: null,
};

function renderDialog(onConfirm: () => Promise<void>, onCancel = vi.fn()) {
  localStorage.setItem('dumply_language', 'en');
  render(
    <LanguageProvider>
      <DeleteConfigDialog config={config} onCancel={onCancel} onConfirm={onConfirm} />
    </LanguageProvider>,
  );
  return { onCancel };
}

describe('DeleteConfigDialog', () => {
  it('prevents duplicate submission and Escape while deletion is pending', async () => {
    let finish!: () => void;
    const onConfirm = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const { onCancel } = renderDialog(onConfirm);
    const user = userEvent.setup();
    const button = screen.getByRole('button', { name: 'Delete Permanently' });

    await user.click(button);

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Deleting…' }));
    expect(onConfirm).toHaveBeenCalledTimes(1);

    const dialog = screen.getByRole('dialog');
    const cancelEvent = new Event('cancel', { cancelable: true });
    fireEvent(dialog, cancelEvent);
    expect(cancelEvent.defaultPrevented).toBe(true);
    expect(onCancel).not.toHaveBeenCalled();

    finish();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete Permanently' })).toBeEnabled());
  });

  it('keeps the dialog open and exposes deletion errors accessibly', async () => {
    renderDialog(vi.fn().mockRejectedValue(new Error('storage is unavailable')));

    await userEvent.click(screen.getByRole('button', { name: 'Delete Permanently' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('storage is unavailable');
    expect(screen.getByRole('dialog')).toHaveAttribute('open');
    expect(screen.getByRole('dialog')).toHaveAttribute('aria-describedby', 'delete-config-description');
    expect(screen.getByRole('button', { name: 'Delete Permanently' })).toBeEnabled();
  });
});
