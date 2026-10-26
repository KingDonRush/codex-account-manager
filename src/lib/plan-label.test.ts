import { describe, it, expect } from 'vitest';
import { planLabel } from './plan-label';
describe('plan presentation never invents entitlement', () => {
  it.each(['Free', 'Go', 'Plus', 'Pro', 'Business', 'Enterprise', 'Edu'])(
    'recognizes %s',
    (name) => {
      expect(planLabel(name.toLowerCase())).toBe(name);
    },
  );
  it('preserves unknown variants and missing plans', () => {
    expect(planLabel('custom_pro_tier')).toBe('custom_pro_tier');
    expect(planLabel(null)).toBe('Não identificado');
    expect(planLabel('team')).toBe('Business (Team)');
  });
});
