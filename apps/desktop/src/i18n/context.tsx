import { createContext, useContext, useEffect, useState, useCallback, type ReactNode } from 'react';
import type { Locale, TranslationKeys } from './types';
import { zhCN } from './locales/zh-CN';
import { en } from './locales/en';
import { ja } from './locales/ja';
import { ko } from './locales/ko';
import { zhTW } from './locales/zh-TW';
import { fr } from './locales/fr';
import { de } from './locales/de';
import { es } from './locales/es';
import { pt } from './locales/pt';
import { ru } from './locales/ru';

const translations: Record<Locale, TranslationKeys> = {
  'zh-CN': zhCN,
  en,
  ja,
  ko,
  'zh-TW': zhTW,
  fr,
  de,
  es,
  pt,
  ru,
};

const STORAGE_KEY = 'nexa-locale';
const LEGACY_STORAGE_KEY = 'ask-myself-locale';

// One-shot migration from pre-Nexa storage key (v0.x rebrand)
if (typeof window !== 'undefined' && window.localStorage) {
  if (!localStorage.getItem(STORAGE_KEY)) {
    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy) {
      localStorage.setItem(STORAGE_KEY, legacy);
      localStorage.removeItem(LEGACY_STORAGE_KEY);
    }
  }
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function detectLocale(): Locale {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved && saved in translations) return saved as Locale;

  const browserLang = navigator.language;
  if (browserLang.startsWith('zh-TW') || browserLang.startsWith('zh-Hant')) return 'zh-TW';
  if (browserLang.startsWith('zh')) return 'zh-CN';
  if (browserLang.startsWith('ja')) return 'ja';
  if (browserLang.startsWith('ko')) return 'ko';
  if (browserLang.startsWith('fr')) return 'fr';
  if (browserLang.startsWith('de')) return 'de';
  if (browserLang.startsWith('es')) return 'es';
  if (browserLang.startsWith('pt')) return 'pt';
  if (browserLang.startsWith('ru')) return 'ru';
  return 'en';
}

interface I18nContextType {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: keyof TranslationKeys, params?: Record<string, string | number>) => string;
  availableLocales: { code: Locale; name: string }[];
}

const I18nContext = createContext<I18nContextType>(null!);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detectLocale);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    localStorage.setItem(STORAGE_KEY, l);
  }, []);

  const t = useCallback((key: keyof TranslationKeys, params?: Record<string, string | number>) => {
    let text = translations[locale]?.[key] ?? translations.en?.[key] ?? key;
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        const escapedKey = escapeRegExp(k);
        text = text.replace(new RegExp(`\\{\\{\\s*${escapedKey}\\s*\\}\\}`, 'g'), String(v));
        text = text.replace(new RegExp(`\\{${escapedKey}\\}`, 'g'), String(v));
      }
    }
    return text;
  }, [locale]);

  const availableLocales: { code: Locale; name: string }[] = [
    { code: 'zh-CN', name: '简体中文' },
    { code: 'zh-TW', name: '繁體中文' },
    { code: 'en', name: 'English' },
    { code: 'ja', name: '日本語' },
    { code: 'ko', name: '한국어' },
    { code: 'fr', name: 'Français' },
    { code: 'de', name: 'Deutsch' },
    { code: 'es', name: 'Español' },
    { code: 'pt', name: 'Português' },
    { code: 'ru', name: 'Русский' },
  ];

  return (
    <I18nContext.Provider value={{ locale, setLocale, t, availableLocales }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useTranslation() {
  return useContext(I18nContext);
}
