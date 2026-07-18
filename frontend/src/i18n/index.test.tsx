import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { LanguageProvider, useLanguage } from './index';

function LanguageProbe({ includeMissing = false }: { includeMissing?: boolean }) {
  const { language, setLanguage, t } = useLanguage();

  return (
    <div>
      <p data-testid="language">{language}</p>
      <p>{t('login.title')}</p>
      <p>{t('history.pagination.page', { page: 3 })}</p>
      {includeMissing && <p>{t('missing.key')}</p>}
      <button onClick={() => setLanguage('en')}>Switch to English</button>
    </div>
  );
}

function setNavigatorLanguage(language: string) {
  Object.defineProperty(window.navigator, 'language', {
    value: language,
    configurable: true,
  });
}

describe('LanguageProvider', () => {
  it('uses English for English browser locales', () => {
    setNavigatorLanguage('en-US');

    render(<LanguageProvider><LanguageProbe /></LanguageProvider>);

    expect(screen.getByTestId('language')).toHaveTextContent('en');
    expect(screen.getByText('Login to Backup Console')).toBeInTheDocument();
    expect(screen.getByText('Page 3')).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute('lang', 'en');
  });

  it('prefers persisted language and stores updates', async () => {
    localStorage.setItem('dumply_language', 'zh-TW');
    setNavigatorLanguage('en-US');

    render(<LanguageProvider><LanguageProbe /></LanguageProvider>);

    expect(screen.getByTestId('language')).toHaveTextContent('zh-TW');
    expect(screen.getByText('登入備份管理台')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Switch to English' }));

    expect(screen.getByTestId('language')).toHaveTextContent('en');
    expect(screen.getByText('Login to Backup Console')).toBeInTheDocument();
    expect(localStorage.getItem('dumply_language')).toBe('en');
    expect(document.documentElement).toHaveAttribute('lang', 'en');
  });

  it('falls back to the key when a translation is missing', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

    render(<LanguageProvider><LanguageProbe includeMissing /></LanguageProvider>);

    expect(screen.getByText('missing.key')).toBeInTheDocument();
    expect(warn).toHaveBeenCalledWith('Translation missing: missing.key');
  });
});
