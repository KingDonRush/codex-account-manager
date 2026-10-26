<script lang="ts">
  import type { IntegrationAccount, AccountGroup } from '../lib/integrations';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import { applicationReadiness } from '../lib/application-access';
  import Icon from './Icon.svelte';
  import PlanBadge from './PlanBadge.svelte';
  export let accounts: IntegrationAccount[];
  export let groups: AccountGroup[];
  export let configure: (account: IntegrationAccount) => void;
  export let connectSession: (profileId: string) => void;
  export let executeAccountIds: string[];
  export let groupId = '';
  let query = '';
  $: scoped = groupId
    ? accounts.filter((account) =>
        groups.find((group) => group.id === groupId)?.accountIds.includes(account.id),
      )
    : accounts;
  $: index = buildSearchIndex(scoped, (account) => [
    account.name,
    account.email ?? '',
    account.plan ?? '',
  ]);
  $: matches = searchIndex(index, query);
</script>

<div class="authorized-resources">
  <div class="resource-tools">
    <label class="search-field"
      ><Icon name="search" size={16} /><input
        aria-label="Buscar contas autorizadas"
        placeholder="Buscar conta ou plano"
        bind:value={query}
      /></label
    >
  </div>
  {#each matches as account (account.id)}
    {@const state = applicationReadiness(account)}
    {@const canExecute = executeAccountIds.includes(account.id)}
    <div class="authorized-resource" role="group" aria-label={account.name}>
      <span class="resource-avatar"><Icon name="user" size={18} /></span>
      <div class="resource-identity">
        <strong title={account.name}>{account.name}</strong><small
          >{account.email ?? 'E-mail não informado'}</small
        ><span class="resource-meta"
          ><PlanBadge plan={account.plan} /><span
            class:saved={canExecute && state.kind === 'saved'}
            title={canExecute ? state.guidance : 'Execução não foi autorizada neste recurso.'}
            >{canExecute ? state.label : 'Acesso de consulta'}</span
          ></span
        >
      </div>
      <div class="resource-actions">
        {#if canExecute && state.kind === 'login'}<button
            class="button small primary"
            onclick={() => connectSession(account.profileId)}
            ><Icon name="link" size={15} />Conectar</button
          >{:else if canExecute && state.kind === 'policy'}<button
            class="button small primary"
            onclick={() => configure(account)}
            ><Icon name="settings" size={15} />Habilitar uso</button
          >{:else if canExecute && state.kind === 'recovery'}<button
            class="button small primary"
            onclick={() => configure(account)}>Concluir gravação</button
          >{/if}<button class="button small" onclick={() => configure(account)}
          ><Icon name="settings" size={15} />Configurar conta</button
        >
      </div>
    </div>
  {:else}<p class="form-hint">
      {accounts.length
        ? 'Nenhuma conta corresponde à busca e ao filtro.'
        : 'Nenhuma conta autorizada nesta conexão. Solicite o acesso pelo aplicativo.'}
    </p>{/each}
</div>

<style>
  .resource-tools {
    display: flex;
    gap: 10px;
    margin: 4px 0 14px;
    flex-wrap: wrap;
  }
  .search-field {
    flex: 1;
    min-width: 180px;
  }
  .authorized-resource {
    display: grid;
    grid-template-columns: 28px minmax(0, 1fr) auto;
    align-items: center;
    gap: 11px;
    padding: 15px 0;
    border-top: 1px solid var(--line);
  }
  .resource-avatar {
    color: var(--accent);
  }
  .resource-identity {
    min-width: 0;
  }
  strong {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }
  small {
    display: block;
    color: var(--muted);
    font-size: 11px;
    margin-top: 3px;
    overflow-wrap: anywhere;
  }
  .resource-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
    color: var(--muted);
    font-size: 10px;
    flex-wrap: wrap;
  }
  .saved {
    color: var(--muted);
  }
  .resource-actions {
    display: flex;
    gap: 7px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  @media (max-width: 1000px) {
    .authorized-resource {
      grid-template-columns: 24px minmax(0, 1fr);
    }
    .resource-actions {
      grid-column: 2;
      justify-content: flex-start;
    }
  }
</style>
