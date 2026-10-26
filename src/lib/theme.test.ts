import { describe, expect, it } from 'vitest';
import { resolveTheme } from './theme';

describe('theme preference', () => {
  it('follows the system only in automatic mode', () => {
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('light', true)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });
});
