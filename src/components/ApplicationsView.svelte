<script lang="ts">
  import {
    integrationApi,
    type Application,
    type ExecutionBinding,
    type IntegrationSnapshot,
  } from '../lib/integrations';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import Icon from './Icon.svelte';
  import ApplicationAccounts from './ApplicationAccounts.svelte';
  import ApplicationActivity from './ApplicationActivity.svelte';
  import ApplicationEditor from './ApplicationEditor.svelte';
  import BindingEditor from './BindingEditor.svelte';
  import ApplicationResources from './ApplicationResources.svelte';
  import ApplicationAccountModal from './ApplicationAccountModal.svelte';
  import GroupFilter from './GroupFilter.svelte';
  import {
    activeApplicationGrants,
    applicationAccounts,
    executableApplicationAccounts,
  } from '../lib/application-access';
  export let snapshot: IntegrationSnapshot;
  export let changed: () => Promise<void>;
  export let connectSession: (profileId: string) => void;
  export let manageGroups: () => void;
  export let focusApplicationId: string | null = null;
  let configuredAccountId: string | null = null;
  $: configuredAccount = snapshot.accounts.find((account) => account.id === configuredAccountId);
  let query = '';
  let groupId = '';
  let draft: Application | null = null;
  let binding: ExecutionBinding | null = null;
  let error = '';
  let busyGrant = '';
  $: index = buildSearchIndex(snapshot.applications, (app) => [app.name, app.slug]);
  $: applications = searchIndex(index, query);
  function create(): void {
    draft = { id: crypto.randomUUID(), name: '', slug: '', paused: false, revision: 0 };
  }
  async function saveApplication(value: Application): Promise<void> {
    await integrationApi.saveApplication(value);
    await changed();
  }
  async function saveBinding(value: ExecutionBinding): Promise<void> {
    await integrationApi.saveBinding(value);
    await changed();
  }
  function targetName(item: ExecutionBinding): string {
    return item.target.kind === 'group'
      ? (snapshot.groups.find((group) => group.id === item.target.id)?.name ??
          'Grupo não disponível')
      : (snapshot.accounts.find((account) => account.id === item.target.id)?.name ??
          'Conta não disponível');
  }
  async function revoke(id: string): Promise<void> {
    if (!window.confirm('Revogar esta autorização? Novas conexões não poderão usá-la.')) return;
    busyGrant = id;
    error = '';
    try {
      await integrationApi.revoke(id);
      await changed();
    } catch (cause) {
      error = String(cause);
    } finally {
      busyGrant = '';
    }
  }
</script>

<section aria-label="Aplicações e contas autorizadas">
  <div class="accounts-toolbar">
    <label class="search-field"
      ><Icon name="search" size={17} /><input
        data-area-search
        aria-label="Buscar aplicações"
        placeholder="Buscar aplicações"
        bind:value={query}
      /></label
    ><button class="button" onclick={create}
      ><Icon name="plus" size={17} />Cadastrar aplicação</button
    >
  </div>
  <GroupFilter
    groups={snapshot.groups}
    bind:value={groupId}
    label="Filtrar contas de aplicações por grupo"
    manage={manageGroups}
  />
  <p class="section-hint">
    A conexão define as contas e grupos que cada aplicação pode usar. Não é preciso cadastrar
    vínculo.
  </p>
  {#if error}<p class="notice error" role="alert">{error}</p>{/if}
  {#if !applications.length}<div class="application-empty">
      <span class="empty-icon"><Icon name="grid" size={24} /></span>
      <div>
        <h2>{query ? 'Nenhuma aplicação encontrada' : 'Suas aplicações, suas contas'}</h2>
        <p>
          {query
            ? 'Tente outro nome ou identificador.'
            : 'Solicite a conexão pelo aplicativo que você quer usar. Aqui você aprova as contas e acompanha o acesso.'}
        </p>
      </div>
    </div>{/if}
  <div class="provider-list application-list">
    {#each applications as app (app.id)}
      {@const grants = activeApplicationGrants(snapshot, app.id)}
      {@const resources = applicationAccounts(snapshot, app.id)}
      {@const canExecute = grants.some((grant) => grant.scope.permissions.includes('execute'))}
      {@const executeAccountIds = executableApplicationAccounts(snapshot, app.id)}
      <details class="provider-item application-card" open={focusApplicationId === app.id}>
        <summary
          ><span class="app-avatar"><Icon name="grid" size={18} /></span>
          <div class="app-identity"><strong>{app.name}</strong><small>{app.slug}</small></div>
          <span class="app-state"
            >{app.paused
              ? 'Pausada'
              : grants.length
                ? `${resources.length} contas autorizadas`
                : 'Aguardando autorização'}</span
          ><Icon name="chevron-down" size={16} /></summary
        >
        <div class="provider-details">
          <div class="row app-actions">
            <button class="button small" onclick={() => (draft = structuredClone(app))}
              ><Icon name="pencil" size={15} />Editar aplicação</button
            >
          </div>
          {#if grants.length}<p class="app-access-note">
              <Icon name="shield-check" size={15} />{canExecute
                ? 'Acesso concedido. Cada execução escolhe uma das contas elegíveis, sem trocar seu Codex.'
                : 'Acesso de consulta concedido. Execução não foi autorizada.'}
            </p>
            <ApplicationResources
              accounts={resources}
              groups={snapshot.groups}
              {groupId}
              {executeAccountIds}
              {connectSession}
              configure={(account) => (configuredAccountId = account.id)}
            />
          {:else}<p class="form-hint">
              Abra a aplicação e solicite uma conexão. Você escolhe contas, grupos e ações no mesmo
              consentimento.
            </p>{/if}
          {#if snapshot.bindings.some((item) => item.applicationId === app.id)}<details
              class="legacy-settings form-disclosure"
            >
              <summary
                ><Icon name="settings" size={15} />Configurações anteriores (opcional)</summary
              >
              <p class="form-hint">
                Mantidas para clientes antigos que usam bindingId. O uso direto das contas
                autorizadas não depende delas.
              </p>
              {#each snapshot.bindings.filter((item) => item.applicationId === app.id) as item (item.id)}<div
                  class="binding-row"
                >
                  <span class="binding-copy"
                    ><strong>{item.name}</strong><small
                      >{targetName(item)}{item.automatic ? ' · Escolha automática' : ''}{item.paused
                        ? ' · Pausado'
                        : ''}</small
                    ></span
                  ><button class="button small" onclick={() => (binding = structuredClone(item))}
                    ><Icon name="settings" size={15} />Configurar</button
                  >
                </div>
              {/each}
            </details>{/if}
        </div>
      </details>{/each}
  </div>
  <ApplicationAccounts
    {snapshot}
    {changed}
    {connectSession}
    {manageGroups}
    {groupId}
    showFilter={false}
  />
  <ApplicationActivity {snapshot} {changed} />
  {#if snapshot.grants.length}<details class="grants form-disclosure">
      <summary
        ><Icon name="shield-check" size={16} />Autorizações concedidas
        <span>{snapshot.grants.length}</span></summary
      >
      {#each snapshot.grants as grant (grant.id)}<div class="binding-row">
          <span class="binding-copy"
            ><strong
              >{snapshot.applications.find((app) => app.id === grant.applicationId)?.name ??
                'Aplicação'}</strong
            ><small
              >{grant.revoked
                ? 'Revogada'
                : grant.scope.permanent
                  ? 'Permanente'
                  : (grant.scope.expiresAt ?? 'Esta conexão')}</small
            ></span
          >{#if !grant.revoked}<button
              class="button small danger"
              disabled={busyGrant === grant.id}
              onclick={() => revoke(grant.id)}>Revogar</button
            >{/if}
        </div>{/each}
    </details>{/if}
</section>
{#if draft}<ApplicationEditor
    application={draft}
    save={saveApplication}
    close={() => (draft = null)}
  />{/if}
{#if binding}<BindingEditor
    {binding}
    {snapshot}
    save={saveBinding}
    close={() => (binding = null)}
  />{/if}
{#if configuredAccount}<ApplicationAccountModal
    account={configuredAccount}
    {changed}
    {connectSession}
    close={() => (configuredAccountId = null)}
  />{/if}

<style>
  .application-empty {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 24px;
    border: 1px dashed var(--line);
    border-radius: 12px;
    background: var(--surface);
  }
  .application-empty h2 {
    font-size: 16px;
    margin: 0 0 6px;
  }
  .application-empty p {
    font-size: 12px;
    color: var(--muted);
    margin: 0;
    max-width: 62ch;
  }
  .application-list {
    margin-top: 16px;
    border: 0;
    gap: 12px;
  }
  .application-card {
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
    overflow: hidden;
  }
  .application-card summary {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 18px;
    list-style: none;
  }
  .application-card summary::-webkit-details-marker {
    display: none;
  }
  .app-avatar {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 9px;
    background: var(--soft);
    color: var(--accent);
    flex: none;
  }
  .app-identity,
  .binding-copy {
    min-width: 0;
    flex: 1;
  }
  .app-identity strong,
  .binding-copy strong {
    display: block;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 13px;
  }
  small {
    display: block;
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
    overflow-wrap: anywhere;
  }
  .app-state {
    color: var(--muted);
    font-size: 12px;
  }
  .provider-details {
    display: block;
    padding: 0 18px 18px;
  }
  .app-actions {
    margin-bottom: 14px;
  }
  .binding-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    border-top: 1px solid var(--line);
  }
  .app-access-note {
    display: flex;
    align-items: start;
    gap: 7px;
    margin: 0 0 16px;
    color: var(--muted);
    font-size: 11px;
    line-height: 1.6;
  }
  .legacy-settings {
    margin-top: 16px;
  }
  .grants {
    padding: 16px 20px;
    margin-top: 20px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
  }
  .grants summary > span {
    margin-left: auto;
    color: var(--muted);
  }
</style>
