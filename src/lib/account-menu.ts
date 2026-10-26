import type { ManagedProfile } from './types';
import type { MenuGlyph } from './menu-assets';
import { profilePresentation } from './view';

export type AccountMenuAction =
  | 'primary'
  | 'browser'
  | 'mail'
  | 'rename'
  | 'check'
  | 'archive'
  | 'manage'
  | 'reconnect'
  | 'configureMail'
  | 'folder'
  | 'delete';
export interface AccountMenuItem {
  action: AccountMenuAction;
  label: string;
  glyph: MenuGlyph | 'folder-open';
  group: 'use' | 'open' | 'manage' | 'danger';
  shortcut?: string;
  disabled?: boolean;
}

export function accountMenuItems(profile: ManagedProfile, extended = false): AccountMenuItem[] {
  const items = baseItems(profile);
  if (extended) items.push(...secondaryItems(profile));
  const groups = { use: 0, open: 1, manage: 2, danger: 3 };
  items.sort((left, right) => groups[left.group] - groups[right.group]);
  return items;
}

function baseItems(profile: ManagedProfile): AccountMenuItem[] {
  const presentation = profilePresentation(profile);
  const items: AccountMenuItem[] = [
    {
      action: 'primary',
      label: presentation.actionLabel,
      glyph: actionGlyph(presentation.action),
      group: 'use',
      disabled: presentation.action === 'none',
    },
    { action: 'browser', label: 'Abrir perfil', glyph: 'browser', group: 'open' },
    { action: 'mail', label: 'Abrir e-mail', glyph: 'mail', group: 'open' },
    { action: 'rename', label: 'Renomear', glyph: 'rename', group: 'manage', shortcut: 'F2' },
    { action: 'manage', label: 'Gerenciar conta', glyph: 'manage', group: 'manage' },
  ];
  if (presentation.action !== 'check')
    items.splice(1, 0, {
      action: 'check',
      label: 'Verificar conexão',
      glyph: 'checkConnection',
      group: 'use',
    });
  if (presentation.action !== 'restore')
    items.push({
      action: 'archive',
      label: profile.status === 'archived' ? 'Restaurar perfil' : 'Arquivar perfil',
      glyph: profile.status === 'archived' ? 'restore' : 'archive',
      group: 'manage',
    });
  return items;
}

function actionGlyph(action: ReturnType<typeof profilePresentation>['action']): MenuGlyph {
  if (action === 'check') return 'checkSession';
  if (action === 'restore') return 'restore';
  if (action === 'connect') return 'connect';
  if (action === 'reauthenticate') return 'renew';
  return 'switchAccount';
}

function secondaryItems(profile: ManagedProfile): AccountMenuItem[] {
  const items: AccountMenuItem[] = [
    { action: 'configureMail', label: 'Configurar webmail', glyph: 'manage', group: 'open' },
    { action: 'folder', label: 'Abrir pasta', glyph: 'folder-open', group: 'open' },
    { action: 'delete', label: 'Excluir dados locais', glyph: 'remove', group: 'danger' },
  ];
  if (profile.accountId && profile.status === 'active')
    items.unshift({ action: 'reconnect', label: 'Reautenticar', glyph: 'renew', group: 'use' });
  return items;
}
