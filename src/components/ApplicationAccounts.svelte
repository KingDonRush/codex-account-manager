<script lang="ts">
  import type { IntegrationSnapshot } from '../lib/integrations';
  import { groupAccounts } from '../lib/group-filter';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import Icon from './Icon.svelte';
  import GroupFilter from './GroupFilter.svelte';
  import ApplicationAccountModal from './ApplicationAccountModal.svelte';
  import PlanBadge from './PlanBadge.svelte';
  import { applicationSessionPresentation } from '../lib/application-access';
  export let snapshot: IntegrationSnapshot;
  export let changed: () => Promise<void>;
  export let connectSession: (profileId: string) => void;
  export let manageGroups: () => void;
  let expanded = false;
  export let groupId = '';
  export let showFilter = true;
  let query = '';
  let selectedId: string | null = null;
  $: accounts = groupAccounts(snapshot, groupId);
  $: index = buildSearchIndex(accounts, (account) => [account.name, account.email ?? '']);
  $: matches = searchIndex(index, query);
  $: selected = snapshot.accounts.find((account) => account.id === selectedId);
  $: if (groupId && !snapshot.groups.some((group) => group.id === groupId && !group.archived))
    groupId = '';
  const policies = {
    desktop_only: 'Só Codex',
    applications_only: 'Só aplicações',
    shared: 'Codex e aplicações',
  };
</script>

<section class="application-accounts" aria-labelledby="app-accounts-title">
  <div class="section-header">
    <div>
      <h2 id="app-accounts-title"><Icon name="accounts" size={18} />Contas para aplicações</h2>
      <p>Sessão própria e permissão de uso, sem alterar o login do Desktop.</p>
    </div>
    <button class="button small" aria-expanded={expanded} onclick={() => (expanded = !expanded)}
      ><Icon name={expanded ? 'chevron-down' : 'list'} size={16} />{expanded
        ? 'Recolher contas'
        : `Mostrar contas (${accounts.length})`}</button
    >
  </div>
  {#if showFilter}<GroupFilter
      groups={snapshot.groups}
      bind:value={groupId}
      label="Filtrar contas de aplicações por grupo"
      manage={manageGroups}
    />{/if}
  {#if expanded}
    <label class="search-field resource-search"
      ><Icon name="search" size={16} /><input
        aria-label="Buscar contas de aplicações"
        placeholder="Buscar contas por nome ou e-mail"
        bind:value={query}
      /></label
    >
    <div class="resource-list">
      {#each matches as account (account.id)}
        {@const session = applicationSessionPresentation(account)}
        <div class="resource-row" role="group" aria-label={account.name}>
          <span class="resource-avatar"><Icon name="user" size={18} /></span>
          <div class="resource-identity">
            <strong title={account.name}>{account.name}</strong><small
              >{account.email ?? 'E-mail não informado'}</small
            >
            <PlanBadge plan={account.plan} />
          </div>
          <span
            class="resource-state"
            class:warning={session.kind === 'recovery' || session.kind === 'unverified'}
            title={session.guidance}>{session.label}<small>{policies[account.policy]}</small></span
          >
          <button class="button small" onclick={() => (selectedId = account.id)}
            ><Icon name="settings" size={16} />Configurar</button
          >
        </div>
      {:else}<p class="resource-empty">
          Nenhuma conta corresponde à busca e ao grupo selecionados.
        </p>{/each}
    </div>
  {/if}
</section>
{#if selected}<ApplicationAccountModal
    account={selected}
    {changed}
    {connectSession}
    close={() => (selectedId = null)}
  />{/if}

<style>
  .application-accounts {
    margin-top: 24px;
    padding: 20px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
  }
  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 18px;
    margin-bottom: 16px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 15px;
    margin: 0;
  }
  p {
    font-size: 12px;
    color: var(--muted);
    margin: 6px 0 0;
  }
  .resource-search {
    margin: 16px 0 8px;
  }
  .resource-row {
    display: grid;
    grid-template-columns: 32px minmax(0, 1fr) 155px auto;
    align-items: center;
    gap: 12px;
    padding: 14px 0;
    border-top: 1px solid var(--line);
  }
  .resource-avatar {
    color: var(--accent);
  }
  .resource-identity {
    min-width: 0;
  }
  strong,
  small {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  strong {
    font-size: 13px;
  }
  small {
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
  }
  .resource-state {
    font-size: 12px;
  }
  .resource-state.warning {
    color: var(--warning);
  }
  .resource-empty {
    padding: 12px 0;
  }
  @media (max-width: 900px) {
    .section-header {
      flex-wrap: wrap;
    }
    .resource-row {
      grid-template-columns: minmax(0, 1fr) 130px auto;
    }
    .resource-avatar {
      display: none;
    }
  }
  @media (max-width: 740px) {
    .resource-row {
      grid-template-columns: minmax(0, 1fr) auto;
    }
    .resource-state {
      grid-row: 2;
    }
    .resource-row .button {
      grid-column: 2;
      grid-row: 1 / span 2;
    }
  }
</style>
