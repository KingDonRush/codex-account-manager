<script lang="ts">
  import type { AccountCard, ManagedProfile } from '../lib/types';
  import Icon from './Icon.svelte';
  import { formatAccountDate } from '../lib/date-format';
  import PlanBadge from './PlanBadge.svelte';
  export let account: AccountCard | undefined;
  export let current = false;
  export let activate: (profile: ManagedProfile) => void;
  export let select: (profile: ManagedProfile) => void;
</script>

<section class="spotlight" class:current aria-label={current ? 'Conta em uso' : 'Próxima conta recomendada'}>
  <span class="spotlight-label"><Icon name={current ? 'circle-check' : 'arrow-right'} size={15} />{current ? 'Em uso no Codex' : 'Próxima conta'}</span>
  {#if account}
    <div class="spotlight-main">
      <div><button class="spotlight-name" onclick={() => select(account.profile)}>{account.profile.name}</button><PlanBadge plan={account.profile.planType} /></div>
      <strong class="spotlight-balance">{account.effectiveRemaining === null ? '—' : `${Math.round(account.effectiveRemaining)}%`}<small>{account.projectedReset ? 'estimado' : 'disponível'}</small></strong>
    </div>
    {#if !current && account.recommendation}
      <div class="spotlight-bottom"><details><summary>Por que esta conta?</summary><p>{account.recommendation.reason}</p>{#if account.recommendation.nextResetAt}<p>Renovação útil: {formatAccountDate(account.recommendation.nextResetAt)}</p>{/if}</details><button class="button small" onclick={() => activate(account.profile)}>Usar conta <Icon name="arrow-right" size={15} /></button></div>
    {:else}<p class="spotlight-meta">{account.profile.email ?? 'Conta autenticada'}</p>{/if}
  {:else}
    <p class="spotlight-meta">{current ? 'Nenhuma conta ativa identificada.' : 'Nenhuma outra conta elegível neste momento.'}</p>
  {/if}
</section>
