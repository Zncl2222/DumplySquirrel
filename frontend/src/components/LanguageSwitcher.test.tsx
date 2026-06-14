import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { LanguageProvider, useLanguage } from '../i18n';
import { LanguageSwitcher } from './LanguageSwitcher';

function TestShell() {
  const { language, t } = useLanguage();

  return (
    <>
      <LanguageSwitcher />
      <p data-testid="language">{language}</p>
      <h1>{t('login.title')}</h1>
    </>
  );
}

describe('LanguageSwitcher', () => {
  it('renders available languages and switches translations', async () => {
    localStorage.setItem('dumply_language', 'zh-TW');

    render(<LanguageProvider><TestShell /></LanguageProvider>);

    const select = screen.getByRole('combobox', { name: 'Language' });
    expect(select).toHaveValue('zh-TW');
    expect(screen.getByRole('option', { name: '繁體中文' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'English' })).toBeInTheDocument();
    expect(screen.getByText('登入備份管理台')).toBeInTheDocument();

    await userEvent.selectOptions(select, 'en');

    expect(select).toHaveValue('en');
    expect(screen.getByTestId('language')).toHaveTextContent('en');
    expect(screen.getByText('Login to Backup Console')).toBeInTheDocument();
    expect(localStorage.getItem('dumply_language')).toBe('en');
  });
});
