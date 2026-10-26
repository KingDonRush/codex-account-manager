const knownPlans: Record<string, string> = {
  free: 'Free',
  go: 'Go',
  plus: 'Plus',
  pro: 'Pro',
  business: 'Business',
  team: 'Business (Team)',
  enterprise: 'Enterprise',
  edu: 'Edu',
  education: 'Edu',
};
export function planLabel(value: string | null | undefined): string {
  if (!value?.trim()) return 'Não identificado';
  return knownPlans[value.toLowerCase()] ?? value;
}
