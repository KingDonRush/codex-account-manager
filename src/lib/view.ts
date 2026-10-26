import type { ManagedProfile, OperationState, View } from './types';

export const OPERATION_STEPS: ReadonlyArray<{ state: OperationState; label: string }> = [
  { state: 'preparing', label: 'Preparando operação' },
  { state: 'browser_launching', label: 'Abrindo perfil do navegador' },
  { state: 'awaiting_login', label: 'Aguardando login no Codex' },
  { state: 'verifying', label: 'Validando a sessão' },
  { state: 'persisting', label: 'Salvando credencial no Keyring' },
];

const ACTIVE_STATES: ReadonlyArray<OperationState> = [
  'preparing',
  'browser_launching',
  'awaiting_login',
  'awaiting_confirmation',
  'verifying',
  'persisting',
];

const OPERATION_LABELS: Record<OperationState, string> = {
  idle: 'Parado',
  preparing: 'Preparando',
  browser_launching: 'Abrindo navegador',
  awaiting_login: 'Aguardando login',
  awaiting_confirmation: 'Aguardando confirmação',
  verifying: 'Validando',
  persisting: 'Salvando',
  completed: 'Concluído',
  cancelled: 'Cancelado',
  failed: 'Falhou',
  timed_out: 'Tempo esgotado',
};

const VIEW_TITLES: Record<View, string> = {
  dashboard: 'Contas',
  applications: 'Aplicações',
  providers: 'Catálogo de e-mail',
  diagnostics: 'Diagnóstico',
};

export type ProfileAction = 'connect' | 'reauthenticate' | 'switch' | 'check' | 'restore' | 'none';
export type ProfileTone = 'current' | 'ready' | 'warning' | 'disconnected' | 'archived';

export interface ProfilePresentation {
  action: ProfileAction;
  actionLabel: string;
  stateLabel: string;
  tone: ProfileTone;
}

export function isOperationActive(state: OperationState): boolean {
  return ACTIVE_STATES.includes(state);
}

export function operationLabel(state: OperationState): string {
  return OPERATION_LABELS[state];
}

export function viewTitle(view: View): string {
  return VIEW_TITLES[view];
}

export function remainingPercent(usedPercent: number | null): number | null {
  if (usedPercent === null || !Number.isFinite(usedPercent)) return null;
  return Math.round(Math.max(0, Math.min(100, 100 - usedPercent)));
}

export function quotaPeriodLabel(durationMins: number | null): string {
  if (durationMins === 300) return '5 horas';
  if (durationMins === 10080) return 'Semana';
  return durationMins ? `${durationMins} min` : 'Período';
}

export function quotaTone(remaining: number | null): 'good' | 'low' | 'critical' | 'unknown' {
  if (remaining === null) return 'unknown';
  if (remaining <= 15) return 'critical';
  if (remaining <= 35) return 'low';
  return 'good';
}

export function profilePresentation(profile: ManagedProfile): ProfilePresentation {
  if (profile.status === 'archived') {
    return {
      action: 'restore',
      actionLabel: 'Restaurar perfil',
      stateLabel: 'Arquivada',
      tone: 'archived',
    };
  }
  if (!profile.accountId) {
    return {
      action: 'connect',
      actionLabel: 'Conectar conta',
      stateLabel: 'Não conectada',
      tone: 'disconnected',
    };
  }
  if (profile.credentialState === 'quarantined') {
    return {
      action: 'check',
      actionLabel: 'Verificar sessão salva',
      stateLabel: 'Sessão a verificar',
      tone: 'warning',
    };
  }
  if (['missing', 'not_connected'].includes(profile.credentialState)) {
    return {
      action: 'reauthenticate',
      actionLabel: 'Corrigir conexão',
      stateLabel: 'Requer atenção',
      tone: 'warning',
    };
  }
  if (!['ready', 'unavailable'].includes(profile.credentialState)) {
    return {
      action: 'check',
      actionLabel: 'Verificar estado da sessão',
      stateLabel: 'Estado não verificado',
      tone: 'warning',
    };
  }
  if (profile.isActiveAccount && profile.needsActivation) {
    return {
      action: 'switch',
      actionLabel: 'Aplicar conexão renovada',
      stateLabel: 'Conexão renovada',
      tone: 'ready',
    };
  }
  if (profile.isActiveAccount) {
    return {
      action: 'none',
      actionLabel: 'Em uso no Codex',
      stateLabel: 'Em uso',
      tone: 'current',
    };
  }
  if (profile.credentialState === 'unavailable') {
    return {
      action: 'check',
      actionLabel: 'Verificar cofre',
      stateLabel: 'Cofre indisponível',
      tone: 'warning',
    };
  }
  return {
    action: 'switch',
    actionLabel: 'Usar esta conta',
    stateLabel: 'Sessão salva',
    tone: 'ready',
  };
}

export function stepStatus(
  operationState: OperationState | null,
  stepState: OperationState,
): '' | 'current' | 'done' {
  if (!operationState) return '';
  const currentIndex = OPERATION_STEPS.findIndex((item) => item.state === operationState);
  const stepIndex = OPERATION_STEPS.findIndex((item) => item.state === stepState);
  if (operationState === 'completed' || stepIndex < currentIndex) return 'done';
  if (stepState === operationState) return 'current';
  return '';
}
