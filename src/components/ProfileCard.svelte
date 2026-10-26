<script lang="ts">
  import type { AccountCard, ManagedProfile } from '../lib/types';
  import { profilePresentation } from '../lib/view';
  import { formatAccountDate as dateLabel } from '../lib/date-format';
  import Icon from './Icon.svelte';
  import QuotaBar from './QuotaBar.svelte';
  import QuotaLimitsDetail from './QuotaLimitsDetail.svelte';
  import { planLabel } from '../lib/plan-label';
  import PlanBadge from './PlanBadge.svelte';

  export let account: AccountCard;
  export let now: number;
  export let providerName: string;
  export let connect: (profile: ManagedProfile, reconnect: boolean) => void;
  export let switchAccount: (profile: ManagedProfile) => void;
  export let restore: (profile: ManagedProfile) => void;
  export let check: (profile: ManagedProfile) => void;

  export let menu: (event: MouseEvent) => void;
  export let batchMode = false;
  export let batchSelected = false;
  export let toggleBatch: (checked: boolean) => void;
  $: profile = account.profile;
  $: presentation = profilePresentation(profile);

  function quotaAge(value: string | null, currentTime: number): string {
    if (!value) return 'Sem consulta confirmada';
    const minutes = Math.max(0, Math.floor((currentTime - Date.parse(value)) / 60_000));
    if (minutes < 1) return 'Consultado agora';
    if (minutes < 60) return `Consultado há ${minutes} min`;
    if (minutes < 1440) return `Consultado há ${Math.floor(minutes / 60)} h`;
    return `Última consulta há ${Math.floor(minutes / 1440)} dias`;
  }

  function runPrimaryAction(): void {
    if (presentation.action === 'connect') connect(profile, false);
    if (presentation.action === 'reauthenticate') connect(profile, true);
    if (presentation.action === 'switch') switchAccount(profile);
    if (presentation.action === 'check') check(profile);
    if (presentation.action === 'restore') restore(profile);
  }
</script>

<article
  class:archived={profile.status === 'archived'}
  class:current={presentation.tone === 'current'}
  class="account-card"
  oncontextmenu={menu}
>
  {#if account.recommendation}
    <div class="recommendation-badge">
      <Icon name="sparkles" size={15} />{account.recommendation.label}
    </div>
    <p class="recommendation-reason">{account.recommendation.reason}</p>
  {/if}
  <header class="account-head">
    {#if batchMode}<input
        type="checkbox"
        class="card-select"
        aria-label={`Selecionar ${profile.name}`}
        disabled={!profile.accountId}
        checked={batchSelected}
        onchange={(event) => toggleBatch(event.currentTarget.checked)}
      />{/if}
    <div class="account-identity">
      <span class="account-avatar"><Icon name="user" size={22} /></span>
      <div class="account-copy">
        <h2 title={profile.name}>{profile.name}</h2>
        <p>{profile.email ?? 'Nenhuma conta associada'}</p>
        {#if profile.accountId}<PlanBadge plan={profile.planType} />{/if}
      </div>
    </div>
    <div class="account-head-actions">
      <span class={`status-pill ${presentation.tone}`}>
        {#if presentation.tone === 'current'}<Icon name="circle-check" size={14} />
        {:else if presentation.tone === 'ready'}<Icon name="shield-check" size={14} />
        {:else if presentation.tone === 'warning'}<Icon name="triangle-alert" size={14} />
        {:else if presentation.tone === 'archived'}<Icon name="archive" size={14} />
        {:else}<Icon name="link-off" size={14} />{/if}
        {presentation.stateLabel}
      </span>
      <div class="card-menu">
        <button
          class="menu-trigger"
          aria-label={`Mais ações de ${profile.name}`}
          title="Mais ações"
          onclick={menu}
        >
          <Icon name="more" size={20} />
        </button>
      </div>
    </div>
  </header>

  <section class="quota-block" aria-label={`Saldo de ${profile.name}`}>
    <div class="quota-heading">
      <div>
        <span class="eyebrow">Saldo restante{account.projectedReset ? ' · estimado' : ''}</span>
      </div>
      {#if profile.rateLimits?.resetCredits?.availableCount !== null && profile.rateLimits?.resetCredits?.availableCount !== undefined}
        <span class="reset-chip" title="Redefinições de limite disponíveis">
          <Icon name="reset-ticket" size={15} />
          {profile.rateLimits.resetCredits.availableCount}
        </span>
      {/if}
    </div>

    {#if account.primaryQuota || account.secondaryQuota}
      <div class="quota-list">
        {#if account.primaryQuota}
          <QuotaBar window={account.primaryQuota} period="short" {now} />
        {/if}
        {#if account.secondaryQuota}
          <QuotaBar window={account.secondaryQuota} period="weekly" {now} />
        {/if}
      </div>
    {:else}
      <div class="quota-empty">
        <Icon name="gauge" size={19} />
        <span
          >{profile.accountId
            ? 'A consulta de saldo acontece em segundo plano.'
            : 'O saldo aparece depois da conexão.'}</span
        >
      </div>
    {/if}
    {#if profile.accountId}<p class="quota-freshness" title={dateLabel(profile.usageUpdatedAt)}>
        {quotaAge(profile.usageUpdatedAt, now)}
      </p>{/if}
  </section>

  {#if account.health === 'attention'}
    <div class="account-warning">
      <Icon name="info" size={16} />{account.attentionReason ??
        'A conexão precisa ser validada novamente.'}
    </div>
  {/if}

  <div class="account-times" aria-label="Histórico recente">
    <div>
      <span>Último uso</span>
      <strong>{dateLabel(profile.lastUsedAt)}</strong>
    </div>
    <div>
      <span>Login no navegador</span>
      <strong>{dateLabel(profile.lastBrowserAuthenticatedAt)}</strong>
    </div>
  </div>

  <details class="account-details">
    <summary>Mais detalhes <Icon name="chevron-down" size={14} /></summary>
    <dl>
      {#if profile.lastError}<div>
          <dt>Última operação</dt>
          <dd>{profile.lastError}</dd>
        </div>{/if}
      <div>
        <dt>Plano</dt>
        <dd>{planLabel(profile.planType)}</dd>
      </div>
      <div>
        <dt>Última abertura</dt>
        <dd>{dateLabel(profile.lastBrowserOpenedAt)}</dd>
      </div>
      <div>
        <dt>Quota observada</dt>
        <dd>{dateLabel(profile.usageUpdatedAt)}</dd>
      </div>
      <div>
        <dt>Webmail</dt>
        <dd>{providerName}</dd>
      </div>
    </dl>
    <QuotaLimitsDetail limits={profile.rateLimits} {now} />
  </details>

  <footer class="account-footer">
    <button
      class:current-action={presentation.action === 'none'}
      class="primary-action"
      disabled={presentation.action === 'none'}
      onclick={runPrimaryAction}
    >
      {#if presentation.action === 'check'}<Icon name="shield-check" size={18} />
      {:else if presentation.action === 'switch'}<Icon name="log-in" size={18} />
      {:else if presentation.action === 'connect'}<Icon name="link" size={18} />
      {:else if presentation.action === 'reauthenticate'}<Icon name="key-refresh" size={18} />
      {:else if presentation.action === 'restore'}<Icon name="restore" size={18} />
      {:else}<Icon name="circle-check" size={18} />{/if}
      {presentation.actionLabel}
    </button>
    <span class="provider-label" title={providerName}>{providerName}</span>
  </footer>
</article>

<style>
  .card-select {
    width: 16px;
    height: 16px;
    margin: 5px 0 0;
    flex: none;
    padding: 0;
  }
</style>
