import type { TranslationKeys, TranslationNamespace } from './types';

type NamespaceMessages = Record<string, string>;

export function flattenTranslationNamespaces(
  namespaces: Record<TranslationNamespace, NamespaceMessages>,
): TranslationKeys {
  const translations: Record<string, string> = {};

  for (const [namespace, messages] of Object.entries(namespaces)) {
    for (const [key, value] of Object.entries(messages)) {
      translations[`${namespace}.${key}`] = value;
    }
  }

  return translations as TranslationKeys;
}
