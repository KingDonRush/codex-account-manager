import type { IntegrationAccount, IntegrationSnapshot } from './integrations';

/** Public account IDs identify membership; profile IDs only select the local row. */
export function groupAccounts(
  snapshot: IntegrationSnapshot,
  groupId: string,
): IntegrationAccount[] {
  if (!groupId) return snapshot.accounts;
  const group = snapshot.groups.find((item) => item.id === groupId && !item.archived);
  const members = new Set(group?.accountIds ?? []);
  return snapshot.accounts.filter((account) => members.has(account.id));
}
