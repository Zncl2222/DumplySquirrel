import { useLanguage, Language } from '../i18n';

const languages: { value: Language; label: string }[] = [
  { value: 'zh-TW', label: '繁體中文' },
  { value: 'en', label: 'English' },
];

export function LanguageSwitcher() {
  const { language, setLanguage } = useLanguage();

  return (
    <select
      className="language-switcher"
      value={language}
      onChange={(e) => setLanguage(e.target.value as Language)}
      aria-label="Language"
    >
      {languages.map((lang) => (
        <option key={lang.value} value={lang.value}>
          {lang.label}
        </option>
      ))}
    </select>
  );
}
