<script lang="ts">
  import type { AccountCard, ManagedProfile } from '../lib/types';
  import { profilePresentation, quotaTone } from '../lib/view';
  import Icon from './Icon.svelte';
  import PlanBadge from './PlanBadge.svelte';
  export let account: AccountCard;
  export let selected = false;
  export let batchMode = false;
  export let batchSelected = false;
  export let toggleBatch: (checked: boolean) => void = () => {};
  export let select: (profile: ManagedProfile) => void;
  export let activate: (profile: ManagedProfile) => void;
  export let menu: (event: MouseEvent) => void;
  $: profile = account.profile;
  $: presentation = profilePresentation(profile);
  $: remaining = account.effectiveRemaining;
</script>

<div
  class:chosen={selected}
  class:current={profile.isActiveAccount}
  class:batch={batchMode}
  class="account-row"
  oncontextmenu={menu}
  role="group"
  aria-label={profile.name}
>
  {#if batchMode}<input
      type="checkbox"
      class="row-select"
      aria-label={`Selecionar ${profile.name}`}
      checked={batchSelected}
      disabled={!profile.accountId}
      onchange={(event) => toggleBatch(event.currentTarget.checked)}
    />{/if}
  <button
    class="row-identity"
    title={profile.name}
    onclick={() => select(profile)}
    aria-label={`Detalhes de ${profile.name}`}
  >
    <span class="account-avatar"><Icon name="user" size={19} /></span>
    <span class="row-copy"
      ><strong
        >{profile.name}
        {#if profile.preferred}<Icon name="pin" size={12} />{/if}</strong
      ><span class="row-meta"><small>{profile.email ?? 'Conecte uma conta a este perfil'}</small>{#if profile.accountId}<PlanBadge plan={profile.planType} />{/if}</span></span
    >
  </button>
  <span class={`status-pill ${presentation.tone}`}>{presentation.stateLabel}</span>
  <div
    class="row-balance"
    aria-label={`Saldo utilizável de ${profile.name}`}
    title="Capacidade restante na janela mais restrita; abra os detalhes para ver 5 horas e semana separadamente."
  >
    <span
      ><strong>{remaining === null ? '—' : `${Math.round(remaining)}%`}</strong><small
        >{account.projectedReset ? 'estimado' : 'restante'}</small
      ></span
    >
    <div class={`quota-track ${quotaTone(remaining)}`}>
      <span style={`width:${remaining ?? 0}%`}></span>
    </div>
  </div>
  <div class="row-actions">
    <button
      class="button small"
      aria-label={presentation.actionLabel}
      disabled={presentation.action === 'none'}
      onclick={() => activate(profile)}
      >{presentation.action === 'none'
        ? 'Em uso'
        : presentation.action === 'check'
          ? 'Verificar'
          : presentation.action === 'switch'
            ? 'Usar'
            : presentation.action === 'connect'
              ? 'Conectar'
              : presentation.action === 'restore'
                ? 'Restaurar'
                : 'Corrigir'}</button
    >
    <button class="menu-trigger" aria-label={`Mais ações de ${profile.name}`} onclick={menu}
      ><Icon name="more" size={18} /></button
    >
  </div>
</div>

<style>
  .row-meta { display:flex; align-items:center; gap:7px; min-width:0; }
  .row-meta small { min-width:0; }
</style>
