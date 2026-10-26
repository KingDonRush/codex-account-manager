import { describe, expect, it } from 'vitest';
import { buildSearchIndex, normalizeSearch, searchIndex } from './search';

const entries = ['Gmail', 'Yahoo', 'Outlook', 'Juliette Oliveira', 'João da Silva'];
const index = buildSearchIndex(entries, (entry) => [entry]);
describe('shared candidate search', () => {
  it.each([
    ['gmial', 'Gmail'],
    ['yaho', 'Yahoo'],
    ['outlok', 'Outlook'],
    ['juliete', 'Juliette Oliveira'],
    ['  SILVA JOAO ', 'João da Silva'],
  ])('finds %s without changing identity', (query, expected) => {
    expect(searchIndex(index, query)).toContain(expected);
  });
  it('normalizes accents without mutating metadata', () => {
    expect(normalizeSearch('  JOÃO   Silva ')).toBe('joao silva');
    expect(entries[4]).toBe('João da Silva');
  });
  it('prefers exact and rejects unrelated short guesses', () => {
    expect(
      searchIndex(
        buildSearchIndex(['Gmaill', 'Gmail'], (name) => [name]),
        'gmail',
      )[0],
    ).toBe('Gmail');
    expect(searchIndex(index, 'zz')).toEqual([]);
  });
});
