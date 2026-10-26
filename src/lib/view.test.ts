import { describe, expect, it } from 'vitest';
import {
  isOperationActive,
  operationLabel,
  profilePresentation,
  quotaTone,
  remainingPercent,
  stepStatus,
  viewTitle,
} from './view';

describe('operation presentation', () => {
  it('distinguishes active and terminal states', () => {
    expect(isOperationActive('awaiting_login')).toBe(true);
    expect(isOperationActive('completed')).toBe(false);
    expect(isOperationActive('cancelled')).toBe(false);
  });

  it('marks completed and current steps without skipping ahead', () => {
    expect(stepStatus('verifying', 'preparing')).toBe('done');
    expect(stepStatus('verifying', 'verifying')).toBe('current');
    expect(stepStatus('verifying', 'persisting')).toBe('');
  });

  it('provides exhaustive labels for UI states', () => {
    expect(operationLabel('timed_out')).toBe('Tempo esgotado');
    expect(viewTitle('providers')).toBe('Catálogo de e-mail');
  });

  it('presents quota as remaining capacity', () => {
    expect(remainingPercent(38)).toBe(62);
    expect(remainingPercent(125)).toBe(0);
    expect(remainingPercent(null)).toBeNull();
    expect(quotaTone(14)).toBe('critical');
    expect(quotaTone(62)).toBe('good');
  });

  it('offers one unambiguous primary account action', () => {
    const profile = {
      status: 'active',
      accountId: 'account-a',
      isActiveAccount: false,
      lastError: null,
      credentialState: 'ready',
    } as Parameters<typeof profilePresentation>[0];
    expect(profilePresentation(profile).actionLabel).toBe('Usar esta conta');
    expect(profilePresentation({ ...profile, isActiveAccount: true }).action).toBe('none');
    expect(
      profilePresentation({ ...profile, isActiveAccount: true, needsActivation: true }).actionLabel,
    ).toBe('Aplicar conexão renovada');
    expect(profilePresentation({ ...profile, credentialState: 'quarantined' }).actionLabel).toBe(
      'Verificar sessão salva',
    );
    expect(profilePresentation({ ...profile, credentialState: 'quarantined' }).action).toBe(
      'check',
    );
    expect(profilePresentation({ ...profile, lastError: 'rede indisponível' }).action).toBe(
      'switch',
    );
    expect(profilePresentation({ ...profile, credentialState: 'unavailable' }).actionLabel).toBe(
      'Verificar cofre',
    );
    expect(profilePresentation({ ...profile, credentialState: 'unavailable' }).action).toBe(
      'check',
    );
    expect(profilePresentation({ ...profile, lastError: 'operação cancelada' }).action).toBe(
      'switch',
    );
    expect(profilePresentation({ ...profile, credentialState: 'missing' }).tone).toBe('warning');
  });

  it('does not promote a saved credential or an unknown state to verified readiness', () => {
    const saved = {
      status: 'active',
      accountId: 'fixture',
      isActiveAccount: false,
      credentialState: 'ready',
      usageUpdatedAt: null,
    } as Parameters<typeof profilePresentation>[0];
    expect(profilePresentation(saved).stateLabel).toBe('Sessão salva');
    const unknown = JSON.parse(
      JSON.stringify({ ...saved, credentialState: 'future_state' }),
    ) as Parameters<typeof profilePresentation>[0];
    expect(profilePresentation(unknown)).toMatchObject({
      action: 'check',
      stateLabel: 'Estado não verificado',
      tone: 'warning',
    });
    expect(profilePresentation({ ...unknown, isActiveAccount: true }).action).toBe('check');
  });
});
