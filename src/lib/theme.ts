export type ThemePreference = 'system' | 'light' | 'dark';
const storageKey = 'codex-manager.theme';

export function readTheme(): ThemePreference {
  try {
    const saved = localStorage.getItem(storageKey);
    if (saved === 'light' || saved === 'dark') return saved;
  } catch { /* The system theme remains usable if storage is unavailable. */ }
  return 'system';
}

export function resolveTheme(preference: ThemePreference, darkSystem: boolean): 'light' | 'dark' {
  return preference === 'system' ? (darkSystem ? 'dark' : 'light') : preference;
}

export function applyTheme(preference: ThemePreference): void {
  document.documentElement.dataset.theme = resolveTheme(preference, matchMedia('(prefers-color-scheme: dark)').matches);
}

export function saveTheme(preference: ThemePreference): void {
  try { localStorage.setItem(storageKey, preference); } catch { /* Apply the choice for this session regardless of storage. */ }
  applyTheme(preference);
}
