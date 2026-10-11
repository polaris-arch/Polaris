import { autoSelect as zhCN } from '@/i18n/locales/zh-CN.json';
import { autoSelect as zhTW } from '@/i18n/locales/zh-TW.json';
import { autoSelect as enUS } from '@/i18n/locales/en-US.json';
import { autoSelect as ru } from '@/i18n/locales/ru.json';
import { autoSelect as fa } from '@/i18n/locales/fa.json';
import { resolveAuxLanguage } from '@/i18n/auxiliary';
import type { AutoSelectT } from '@/lib/auto-select-actions';

// The independent tray imports only this namespace, never the main-window i18next/store.
const bundles = { 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ru, fa };
function lookup(bundle: unknown, key: string): string | null {
  let value = bundle;
  for (const part of key.split('.')) value = value && typeof value === 'object' ? (value as Record<string, unknown>)[part] : null;
  return typeof value === 'string' ? value : null;
}
export const autoSelectT: AutoSelectT = (key, vars) => {
  const path = key.replace(/^autoSelect\./, '');
  const value = lookup(bundles[resolveAuxLanguage()], path) ?? lookup(enUS, path) ?? key;
  return value.replace(/\{\{(\w+)\}\}/g, (match, name: string) => vars && name in vars ? String(vars[name]) : match);
};
