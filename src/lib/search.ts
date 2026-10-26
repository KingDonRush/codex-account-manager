export interface SearchEntry<T> {
  item: T;
  fields: string[];
  order: number;
}

export function normalizeSearch(value: string): string {
  return value
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLocaleLowerCase('pt-BR')
    .replace(/\s+/g, ' ')
    .trim();
}

export function buildSearchIndex<T>(
  items: readonly T[],
  fields: (item: T) => readonly string[],
): SearchEntry<T>[] {
  return items.map((item, order) => ({ item, order, fields: fields(item).map(normalizeSearch) }));
}

/** Suggests candidates; callers still act on exact, stable IDs. */
export function searchIndex<T>(index: readonly SearchEntry<T>[], query: string): T[] {
  const normalized = normalizeSearch(query).slice(0, 120);
  if (!normalized) return index.map((entry) => entry.item);
  const tokens = normalized.split(' ');
  return index
    .map((entry) => ({ entry, score: entryScore(entry.fields, normalized, tokens) }))
    .filter((result) => result.score !== null)
    .sort(
      (left, right) =>
        (left.score ?? 0) - (right.score ?? 0) || left.entry.order - right.entry.order,
    )
    .map((result) => result.entry.item);
}

function entryScore(fields: string[], query: string, tokens: string[]): number | null {
  if (fields.includes(query)) return 0;
  if (fields.some((field) => field.startsWith(query))) return 10;
  let total = 0;
  for (const token of tokens) {
    const scores = fields
      .map((field) => tokenScore(field, token))
      .filter((score): score is number => score !== null);
    if (!scores.length) return null;
    total += Math.min(...scores);
  }
  return 20 + total;
}

function tokenScore(field: string, token: string): number | null {
  if (field.split(/[\s@._-]+/).includes(token)) return 0;
  if (field.includes(token)) return 10;
  const allowed = token.length < 4 ? 0 : token.length < 8 ? 1 : 2;
  if (!allowed) return null;
  for (const word of field.split(/[\s@._-]+/)) {
    if (Math.abs(word.length - token.length) > allowed) continue;
    if (editDistance(word.slice(0, 64), token) <= allowed) return 30;
  }
  return null;
}

function editDistance(left: string, right: string): number {
  const rows = Array.from({ length: left.length + 1 }, (_, row) =>
    Array.from({ length: right.length + 1 }, (_, column) => (row ? (column ? 0 : row) : column)),
  );
  const cell = (row: number, column: number): number =>
    rows[row]?.[column] ?? Number.POSITIVE_INFINITY;
  for (let row = 1; row <= left.length; row += 1) {
    const current = rows[row];
    if (!current) return Number.POSITIVE_INFINITY;
    for (let column = 1; column <= right.length; column += 1) {
      current[column] = Math.min(
        cell(row - 1, column) + 1,
        cell(row, column - 1) + 1,
        cell(row - 1, column - 1) + (left[row - 1] === right[column - 1] ? 0 : 1),
      );
      if (
        row > 1 &&
        column > 1 &&
        left[row - 1] === right[column - 2] &&
        left[row - 2] === right[column - 1]
      ) {
        current[column] = Math.min(cell(row, column), cell(row - 2, column - 2) + 1);
      }
    }
  }
  return cell(left.length, right.length);
}
