import { createContext, useContext, useEffect, useState, useCallback, type ReactNode } from 'react';
import type { Locale, TranslationKeys } from './types';
import { en } from './locales/en';

const localeLoaders: Record<Locale, () => Promise<TranslationKeys>> = {
  'zh-CN': () => import('./locales/zh-CN').then((module) => module.zhCN),
  en: async () => en,
  ja: () => import('./locales/ja').then((module) => module.ja),
  ko: () => import('./locales/ko').then((module) => module.ko),
  'zh-TW': () => import('./locales/zh-TW').then((module) => module.zhTW),
  fr: () => import('./locales/fr').then((module) => module.fr),
  de: () => import('./locales/de').then((module) => module.de),
  es: () => import('./locales/es').then((module) => module.es),
  pt: () => import('./locales/pt').then((module) => module.pt),
  ru: () => import('./locales/ru').then((module) => module.ru),
};

const STORAGE_KEY = 'nexa-locale';

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function isSupportedLocale(value: string): value is Locale {
  return value in localeLoaders;
}

function detectLocale(): Locale {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved && isSupportedLocale(saved)) return saved;

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
  const [translations, setTranslations] = useState<Partial<Record<Locale, TranslationKeys>>>({ en });

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  useEffect(() => {
    if (translations[locale]) return;

    let cancelled = false;
    localeLoaders[locale]()
      .then((loaded) => {
        if (cancelled) return;
        setTranslations((current) => ({ ...current, [locale]: loaded }));
      })
      .catch(() => {
        if (cancelled) return;
        setTranslations((current) => ({ ...current, [locale]: en }));
      });

    return () => {
      cancelled = true;
    };
  }, [locale, translations]);

  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    localStorage.setItem(STORAGE_KEY, l);
  }, []);

  const t = useCallback((key: keyof TranslationKeys, params?: Record<string, string | number>) => {
    let text = translations[locale]?.[key] ?? en[key] ?? key;
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        const escapedKey = escapeRegExp(k);
        text = text.replace(new RegExp(`\\{\\{\\s*${escapedKey}\\s*\\}\\}`, 'g'), String(v));
        text = text.replace(new RegExp(`\\{${escapedKey}\\}`, 'g'), String(v));
      }
    }
    return text;
  }, [locale, translations]);

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
