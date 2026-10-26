<script lang="ts">
  import type { AccountCard, ManagedProfile, ProfileOptions } from '../lib/types';
  import { formatAccountDate as dateLabel } from '../lib/date-format';
  import Icon from './Icon.svelte';
  import QuotaBar from './QuotaBar.svelte';
  import QuotaLimitsDetail from './QuotaLimitsDetail.svelte';
  import { planLabel } from '../lib/plan-label';
  import PlanBadge from './PlanBadge.svelte';
  export let account: AccountCard;
  export let now: number;
  export let openMail: (profile: ManagedProfile) => void;
  export let configureMail: (profile: ManagedProfile) => void;
  export let rename: (profile: ManagedProfile) => void;
  export let openFolder: (profile: ManagedProfile) => void;
  export let openBrowser: (profile: ManagedProfile) => void;
  export let connect: (profile: ManagedProfile, reconnect: boolean) => void;
  export let archive: (profile: ManagedProfile) => void;
  export let restore: (profile: ManagedProfile) => void;
  export let manage: (profile: ManagedProfile) => void;
  export let check: (profile: ManagedProfile) => void;
  export let saveOptions: (profile: ManagedProfile, options: ProfileOptions) => void;
  export let close: () => void;
  $: profile = account.profile;
</script>

<aside class="account-detail" aria-label={`Detalhes de ${profile.name}`}>
  <header class="detail-header">
    <div>
      <span class="page-kicker">Perfil</span>
      <h2>{profile.name}</h2>
      <p>{profile.email ?? 'Nenhuma conta associada'}</p>
      {#if profile.accountId}<PlanBadge plan={profile.planType} />{/if}
    </div>
    <button class="menu-trigger" aria-label="Fechar detalhes" onclick={close}
      ><Icon name="circle-x" size={19} /></button
    >
  </header>
  {#if account.health === 'attention'}<p class="detail-alert">
      <Icon name="info" size={16} />{account.attentionReason}
    </p>{/if}
  <section class="detail-balance">
    <h3>Saldo restante{account.projectedReset ? ' · estimado' : ''}</h3>
    {#if account.primaryQuota}<QuotaBar window={account.primaryQuota} period="short" {now} />{/if}
    {#if account.secondaryQuota}<QuotaBar
        window={account.secondaryQuota}
        period="weekly"
        {now}
      />{/if}
    {#if !account.primaryQuota && !account.secondaryQuota}<p class="subtitle">
        Ainda sem saldo confirmado.
      </p>{/if}
    <p class="form-hint">Última consulta: {dateLabel(profile.usageUpdatedAt)}</p>
    <QuotaLimitsDetail limits={profile.rateLimits} {now} />
  </section>
  <div class="detail-quick-actions">
    <button class="button small" onclick={() => openBrowser(profile)}
      ><Icon name="browser" size={16} />Abrir perfil</button
    ><button class="button small" onclick={() => openMail(profile)}
      ><Icon name="mail" size={16} />E-mail</button
    >
  </div>
  <details class="detail-section">
    <summary>Rotação <Icon name="chevron-down" size={15} /></summary>
    <div class="detail-fields">
      <label class="check-label"
        ><input
          type="checkbox"
          checked={profile.rotationEnabled}
          onchange={(event) =>
            saveOptions(profile, {
              rotationEnabled: event.currentTarget.checked,
              preferred: profile.preferred,
            })}
        />Incluir na recomendação</label
      >
      <label class="check-label"
        ><input
          type="checkbox"
          checked={profile.preferred}
          onchange={(event) =>
            saveOptions(profile, {
              rotationEnabled: profile.rotationEnabled,
              preferred: event.currentTarget.checked,
            })}
        />Preferir quando houver saldo</label
      >
      <p class="form-hint">Não troca contas automaticamente.</p>
    </div>
  </details>
  <details class="detail-section">
    <summary>Histórico e conexão <Icon name="chevron-down" size={15} /></summary>
    <div class="detail-fields">
      <dl class="detail-history">
        <div>
          <dt>Último uso</dt>
          <dd>{dateLabel(profile.lastUsedAt)}</dd>
        </div>
        <div>
          <dt>Ativada no Codex</dt>
          <dd>{dateLabel(profile.lastActivatedAt)}</dd>
        </div>
        <div>
          <dt>Navegador aberto</dt>
          <dd>{dateLabel(profile.lastBrowserOpenedAt)}</dd>
        </div>
        <div>
          <dt>Login confirmado</dt>
          <dd>{dateLabel(profile.lastBrowserAuthenticatedAt)}</dd>
        </div>
        <div>
          <dt>Plano</dt>
          <dd>{planLabel(profile.planType)}</dd>
        </div>
      </dl>
      {#if profile.lastError}<p class="detail-alert">Último erro: {profile.lastError}</p>{/if}
      {#if profile.accountId}<button class="button small" onclick={() => check(profile)}
          ><Icon name="shield-check" size={16} />Verificar conexão e saldo</button
        ><button class="button small" onclick={() => connect(profile, true)}
          ><Icon name="key-refresh" size={16} />Reautenticar no navegador</button
        >{/if}
    </div>
  </details>
  <details class="detail-section">
    <summary>Gerenciar perfil <Icon name="chevron-down" size={15} /></summary>
    <div class="detail-fields">
      <button class="detail-action" onclick={() => rename(profile)}
        ><Icon name="pencil" size={16} />Renomear perfil</button
      >
      <button class="detail-action" onclick={() => configureMail(profile)}
        ><Icon name="mail" size={16} />Configurar caixa de entrada</button
      >
      <button class="detail-action" onclick={() => openFolder(profile)}
        ><Icon name="folder-open" size={16} />Abrir pasta local</button
      >
      <button
        class="detail-action"
        onclick={() => (profile.status === 'archived' ? restore(profile) : archive(profile))}
        ><Icon name="archive" size={16} />{profile.status === 'archived'
          ? 'Restaurar'
          : 'Arquivar'}</button
      >
      <button class="detail-action" onclick={() => manage(profile)}
        ><Icon name="settings" size={16} />Gerenciar conta</button
      >
    </div>
  </details>
</aside>
