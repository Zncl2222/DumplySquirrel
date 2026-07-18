import React, { createContext, useContext, useState, useCallback, useEffect, ReactNode } from 'react';
import zhTW from './locales/zh-TW.json';
import en from './locales/en.json';

export type Language = 'zh-TW' | 'en';

const translations: Record<Language, typeof zhTW> = {
  'zh-TW': zhTW,
  en: en,
};

interface LanguageContextType {
  language: Language;
  setLanguage: (lang: Language) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

const LanguageContext = createContext<LanguageContextType | null>(null);

const LANGUAGE_STORAGE_KEY = 'dumply_language';

function getInitialLanguage(): Language {
  const stored = localStorage.getItem(LANGUAGE_STORAGE_KEY);
  if (stored === 'en' || stored === 'zh-TW') {
    return stored;
  }
  const browserLang = navigator.language;
  if (browserLang.startsWith('en')) {
    return 'en';
  }
  return 'zh-TW';
}

function getNestedValue(obj: unknown, path: string): string | undefined {
  const keys = path.split('.');
  let current: unknown = obj;
  for (const key of keys) {
    if (current === null || current === undefined) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[key];
  }
  return typeof current === 'string' ? current : undefined;
}

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<Language>(getInitialLanguage);

  useEffect(() => {
    document.documentElement.lang = language === 'en' ? 'en' : 'zh-Hant';
  }, [language]);

  const setLanguage = useCallback((lang: Language) => {
    setLanguageState(lang);
    localStorage.setItem(LANGUAGE_STORAGE_KEY, lang);
  }, []);

  const t = useCallback((key: string, params?: Record<string, string | number>): string => {
    const value = getNestedValue(translations[language], key);
    if (value === undefined) {
      console.warn(`Translation missing: ${key}`);
      return key;
    }
    if (!params) {
      return value;
    }
    return Object.entries(params).reduce(
      (result, [paramKey, paramValue]) => result.replace(new RegExp(`\\{${paramKey}\\}`, 'g'), String(paramValue)),
      value,
    );
  }, [language]);

  return (
    <LanguageContext.Provider value={{ language, setLanguage, t }}>
      {children}
    </LanguageContext.Provider>
  );
}

export function useLanguage() {
  const context = useContext(LanguageContext);
  if (!context) {
    throw new Error('useLanguage must be used within a LanguageProvider');
  }
  return context;
}
