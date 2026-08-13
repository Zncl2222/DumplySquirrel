import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { RunRoomDetail } from './App';
import { BackupHistory } from './api';
import { LanguageProvider } from './i18n';

const runningHistory: BackupHistory = {
  id: 'run-1',
  config_id: 'config-1',
  status: 'running',
  file_name: null,
  file_size: null,
  is_downloadable: false,
  error_message: null,
  started_at: '2026-08-13T00:00:00Z',
  completed_at: null,
  triggered_by: 'manual',
};

function renderRun(status = 'running', cancellationRequested = false, onCancel = vi.fn()) {
  localStorage.setItem('dumply_language', 'en');
  const history = {
    ...runningHistory,
    status,
    completed_at: status === 'running' ? null : '2026-08-13T00:01:00Z',
  };
  const view = render(
    <LanguageProvider>
      <RunRoomDetail
        history={history}
        configName="Production database"
        events={[]}
        loading={false}
        cancellationRequested={cancellationRequested}
        onBack={vi.fn()}
        onCancel={onCancel}
      />
    </LanguageProvider>,
  );
  return { ...view, onCancel };
}

describe('RunRoomDetail cancellation', () => {
  it('offers cancellation for a running backup', async () => {
    const { onCancel } = renderRun();

    await userEvent.click(screen.getByRole('button', { name: 'Cancel backup' }));

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('disables duplicate cancellation requests and hides the action after completion', () => {
    const view = renderRun('running', true);
    expect(screen.getByRole('button', { name: 'Cancellation requested' })).toBeDisabled();

    view.unmount();
    renderRun('cancelled');
    expect(screen.queryByRole('button', { name: 'Cancel backup' })).not.toBeInTheDocument();
  });
});
