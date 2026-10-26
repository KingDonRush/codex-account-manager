<script lang="ts">
  import {
    integrationApi,
    type AuthorizationRequest,
    type GrantScope,
    type IntegrationSnapshot,
    type Permission,
  } from '../lib/integrations';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import {
    effectiveScopeAccounts,
    applicationSessionPresentation,
  } from '../lib/application-access';
  import { dialogFocus } from '../lib/dialog-focus';
  import Icon from './Icon.svelte';
  import PlanBadge from './PlanBadge.svelte';
  export let request: AuthorizationRequest;
  export let snapshot: IntegrationSnapshot;
  export let closed: () => void;
  export let prepared: (applicationId: string | null) => void;
  let scope: GrantScope = {
    ...structuredClone(request.request.requestedScope),
    directAccountIds: [...request.request.requestedScope.accountIds],
    permanent: false,
    expiresAt: null,
    globalIncludingFuture: false,
    backgroundGroupIds: [],
  };
  scope.permissions = scope.permissions.filter(
    (permission) => permission === 'read_metadata' || permission === 'execute',
  );
  let duration = 'connection';
  let until = '';
  let query = '';
  let error = '';
  let busy = false;
  let approvedId: string | null = null;
  const primary: { id: Permission; label: string; note: string }[] = [
    {
      id: 'read_metadata',
      label: 'Consultar contas e saldo',
      note: 'Somente os recursos selecionados.',
    },
    {
      id: 'execute',
      label: 'Executar com estas contas',
      note: 'Escolhe uma identidade antes de cada trabalho, sem trocar o seu Codex.',
    },
  ];
  const advanced: { id: Permission; label: string }[] = [
    { id: 'manage_groups', label: 'Criar e gerenciar grupos' },
    { id: 'manage_accounts', label: 'Gerenciar contas' },
    { id: 'activate_desktop', label: 'Trocar a conta do Codex' },
    { id: 'manage_bindings', label: 'Gerenciar configurações antigas' },
    { id: 'open_browser', label: 'Abrir navegador e e-mail' },
    { id: 'request_deletion', label: 'Pedir exclusão — sempre exige sua confirmação' },
  ];
  $: index = buildSearchIndex(snapshot.accounts, (account) => [
    account.name,
    account.email ?? '',
    account.plan ?? '',
  ]);
  $: accounts = searchIndex(index, query);
  $: selectedIds = effectiveScopeAccounts(scope, snapshot.groups);
  $: selectedAccounts = snapshot.accounts.filter((account) => selectedIds.has(account.id));
  $: executionAllowed = scope.permissions.includes('execute');
  $: existingApplication = snapshot.applications.find(
    (app) => app.id === request.request.applicationId,
  );
  $: desktopOnly = selectedAccounts.filter((account) => account.policy === 'desktop_only');
  $: missingLogin = selectedAccounts.filter(
    (account) => applicationSessionPresentation(account).kind === 'login',
  );
  $: if (duration === 'connection' && scope.backgroundGroupIds.length)
    scope = { ...scope, backgroundGroupIds: [] };

  function selectAccount(id: string, checked: boolean): void {
    const ids = scope.directAccountIds ?? scope.accountIds;
    const direct = checked ? [...new Set([...ids, id])] : ids.filter((value) => value !== id);
    scope = { ...scope, directAccountIds: direct };
  }
  function selectGroup(id: string, checked: boolean): void {
    scope = {
      ...scope,
      groupIds: checked
        ? [...new Set([...scope.groupIds, id])]
        : scope.groupIds.filter((value) => value !== id),
      backgroundGroupIds: scope.backgroundGroupIds.filter((value) => value !== id),
    };
  }
  function selectPermission(id: Permission, checked: boolean): void {
    scope = {
      ...scope,
      permissions: checked
        ? [...new Set([...scope.permissions, id])]
        : scope.permissions.filter((value) => value !== id),
    };
  }
  function selectedByGroup(id: string): boolean {
    return snapshot.groups.some(
      (group) => scope.groupIds.includes(group.id) && group.accountIds.includes(id),
    );
  }
  async function answer(allow: boolean): Promise<void> {
    if (approvedId) {
      await openApprovedAccess();
      return;
    }
    busy = true;
    error = '';
    try {
      if (!allow) {
        await integrationApi.deny(request.id);
        closed();
        return;
      }
      const expiresAt = duration === 'until' ? new Date(until).toISOString() : null;
      if (expiresAt && Date.parse(expiresAt) <= Date.now())
        throw new Error('Escolha uma validade no futuro.');
      approvedId = await integrationApi.approve(
        request.id,
        { ...scope, accountIds: [...selectedIds], expiresAt, permanent: duration === 'permanent' },
        {
          sharedAccountIds: executionAllowed ? desktopOnly.map((account) => account.id) : [],
        },
      );
      await openApprovedAccess();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
  async function openApprovedAccess(): Promise<void> {
    busy = true;
    try {
      const result = await integrationApi.snapshot();
      const application =
        result.grants.find((grant) => grant.id === approvedId)?.applicationId ??
        request.request.applicationId;
      closed();
      prepared(application);
    } catch {
      error =
        'Acesso já autorizado. A atualização da tela falhou; tente abrir as contas novamente. Não é necessário autorizar de novo.';
    } finally {
      busy = false;
    }
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal consent-modal"
    tabindex="-1"
    use:dialogFocus={{
      escape: () => {
        if (!busy) {
          if (approvedId) closed();
          else void answer(false);
        }
      },
    }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="consent-title"
  >
    <div class="modal-title-row">
      <span class="modal-icon"><Icon name="shield-check" size={23} /></span>
      <div>
        <h2 id="consent-title">
          {request.request.applicationId ? 'Alterar acesso de' : 'Conectar'}
          {existingApplication?.name ?? request.request.name}
        </h2>
        <p class="subtitle">
          {request.request.applicationId
            ? 'Nova solicitação para a aplicação existente. Revise o acesso antes de permitir.'
            : 'Escolha o acesso uma vez. Contas já conectadas não precisam de outro login.'}
        </p>
      </div>
    </div>
    <div class="consent-content">
      <div class="request-code">
        <Icon name="lock" size={17} /><span
          >Confira no aplicativo<strong>{request.confirmationCode}</strong></span
        ><small>Nome declarado, não publisher verificado</small>
      </div>
      <section class="consent-section" aria-labelledby="resources-title">
        <div class="section-title">
          <h3 id="resources-title">Contas e grupos</h3>
          <span
            >{selectedIds.size}
            {selectedIds.size === 1 ? 'conta selecionada' : 'contas selecionadas'}</span
          >
        </div>
        {#if snapshot.groups.some((group) => !group.archived)}<div class="group-choices">
            {#each snapshot.groups.filter((group) => !group.archived) as group (group.id)}<label
                class:chosen={scope.groupIds.includes(group.id)}
                ><input
                  type="checkbox"
                  checked={scope.groupIds.includes(group.id)}
                  disabled={busy}
                  onchange={(event) => selectGroup(group.id, event.currentTarget.checked)}
                /><Icon name="accounts" size={16} /><span
                  >{group.name}<small>{group.accountIds.length} contas</small></span
                ></label
              >{/each}
          </div>{/if}
        <label class="search-field"
          ><Icon name="search" size={16} /><input
            aria-label="Buscar contas para autorização"
            placeholder="Buscar nome, e-mail ou plano"
            bind:value={query}
            disabled={busy}
          /></label
        >
        <div class="consent-resources">
          {#each accounts as account (account.id)}<label
              class="consent-account"
              class:chosen={selectedIds.has(account.id)}
            >
              <input
                type="checkbox"
                aria-label={`Autorizar ${account.name}`}
                checked={selectedIds.has(account.id)}
                disabled={busy || selectedByGroup(account.id)}
                onchange={(event) => selectAccount(account.id, event.currentTarget.checked)}
              />
              <span class="identity"
                ><strong>{account.name}</strong><small
                  >{account.email ?? 'E-mail não informado'}{selectedByGroup(account.id)
                    ? ' · Incluída pelo grupo'
                    : ''}</small
                ></span
              ><PlanBadge plan={account.plan} />
            </label>{:else}<p class="form-hint">Nenhuma conta corresponde à busca.</p>{/each}
        </div>
      </section>
      <section class="consent-section" aria-labelledby="actions-title">
        <h3 id="actions-title">O que o aplicativo pode fazer</h3>
        {#each primary as action}<label
            class="permission-choice"
            class:chosen={scope.permissions.includes(action.id)}
            ><input
              type="checkbox"
              checked={scope.permissions.includes(action.id)}
              disabled={busy}
              onchange={(event) => selectPermission(action.id, event.currentTarget.checked)}
            /><span
              ><strong>{action.label}</strong><small>{action.note}</small
              >{#if action.id === 'execute' && executionAllowed && desktopOnly.length}<small
                  >Habilita “Codex e aplicações” para {desktopOnly.length}
                  {desktopOnly.length === 1 ? 'conta selecionada' : 'contas selecionadas'}. O Codex
                  continua disponível.</small
                >{/if}</span
            ></label
          >{/each}
        {#if executionAllowed && missingLogin.length}<p class="setup-note">
            <Icon name="info" size={16} /><span
              >{missingLogin.length}
              {missingLogin.length === 1 ? 'conta precisa' : 'contas precisam'} de login próprio para
              execução paralela. O acesso será aprovado agora; o próximo passo abrirá no mesmo contexto,
              sem vínculo.</span
            >
          </p>{/if}
      </section>
      <section class="consent-section duration-section">
        <h3>Por quanto tempo</h3>
        <label
          >Duração<select bind:value={duration} disabled={busy}
            ><option value="connection">Só esta conexão</option><option value="until"
              >Até uma data/horário</option
            ><option value="permanent">Até eu revogar</option></select
          ></label
        >{#if duration === 'until'}<label
            >Válida até<input type="datetime-local" bind:value={until} disabled={busy} /></label
          >{/if}
      </section>
      <details class="form-disclosure advanced-consent">
        <summary><Icon name="settings" size={16} />Permissões avançadas e identidade</summary>
        <p class="form-hint">
          Estas ações não são necessárias para executar normalmente. Não ficam marcadas por padrão.
        </p>
        {#each advanced as action}<label class="permission-choice"
            ><input
              type="checkbox"
              checked={scope.permissions.includes(action.id)}
              disabled={busy}
              onchange={(event) => selectPermission(action.id, event.currentTarget.checked)}
            /><span>{action.label}</span></label
          >{/each}
        <label class="permission-choice"
          ><input type="checkbox" bind:checked={scope.globalIncludingFuture} disabled={busy} /><span
            >Todas as contas e grupos, inclusive futuros</span
          ></label
        >
        {#if scope.globalIncludingFuture}<p class="notice error">
            Acesso amplo às ações marcadas. Não habilita automaticamente o uso de novas contas “Só
            Codex”.
          </p>{/if}
        {#each snapshot.groups.filter((group) => scope.groupIds.includes(group.id)) as group}<label
            class="permission-choice"
            ><input
              type="checkbox"
              checked={scope.backgroundGroupIds.includes(group.id)}
              disabled={busy || duration === 'connection'}
              onchange={(event) =>
                (scope = {
                  ...scope,
                  backgroundGroupIds: event.currentTarget.checked
                    ? [...new Set([...scope.backgroundGroupIds, group.id])]
                    : scope.backgroundGroupIds.filter((id) => id !== group.id),
                })}
            /><span
              >Continuar com “{group.name}” em segundo plano<small
                >Exclusivo deste aplicativo; exige prazo por data ou até revogar.</small
              ></span
            ></label
          >{/each}
        <dl class="request-identity">
          <div>
            <dt>Nome declarado</dt>
            <dd>{request.request.name}</dd>
          </div>
          <div>
            <dt>Processo</dt>
            <dd>{request.peerPid}</dd>
          </div>
          <div>
            <dt>Executável verificado pelo sistema</dt>
            <dd>{request.executable}</dd>
          </div>
        </dl>
        <p class="form-hint">
          O executável pode ser o cliente de integração, não o programa principal. Nenhum token do
          Codex será entregue.
        </p>
      </details>
      {#if error}<p class="notice error" role="alert">{error}</p>{/if}{#if busy}<p
          role="status"
          class="form-hint"
        >
          Gravando o acesso e as escolhas de uso…
        </p>{/if}
    </div>
    <div class="row modal-actions">
      <button class="button" disabled={busy} onclick={() => (approvedId ? closed() : answer(false))}
        >{approvedId ? 'Fechar' : 'Negar'}</button
      ><button
        class="button primary"
        disabled={busy ||
          !scope.permissions.length ||
          (!selectedIds.size && !scope.globalIncludingFuture) ||
          (duration === 'until' && !until)}
        onclick={() => answer(true)}
        ><Icon name="shield-check" size={17} />{approvedId
          ? 'Abrir contas autorizadas'
          : 'Autorizar e concluir acesso'}</button
      >
    </div>
  </div>
</div>

<style>
  .consent-modal {
    max-width: 620px;
    max-height: 90vh;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .consent-content {
    min-height: 0;
    overflow-y: auto;
    padding-right: 6px;
  }
  .modal-actions {
    flex: none;
    padding-top: 14px;
    margin-top: 10px;
    border-top: 1px solid var(--line);
    justify-content: space-between;
  }
  h3 {
    font-size: 13px;
    font-weight: 650;
    margin: 0 0 11px;
  }
  .request-code {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 11px 13px;
    margin: 18px 0;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--soft);
    font-size: 11px;
    color: var(--muted);
  }
  .request-code strong {
    display: block;
    color: var(--ink);
    font-size: 14px;
    letter-spacing: 0.13em;
    margin-top: 3px;
  }
  .request-code > small {
    margin-left: auto;
    font-size: 10px;
    max-width: 140px;
    text-align: right;
  }
  .consent-section {
    padding: 18px 0;
    border-top: 1px solid var(--line);
  }
  .consent-modal .search-field {
    display: flex;
    margin: 0;
    align-items: center;
    gap: 9px;
  }
  .consent-modal .search-field input {
    border: 0;
    background: transparent;
    padding: 9px 0;
  }
  .section-title {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 10px;
  }
  .section-title > span {
    color: var(--muted);
    font-size: 11px;
  }
  .consent-resources {
    max-height: 195px;
    overflow: auto;
    margin-top: 8px;
  }
  .consent-account,
  .permission-choice,
  .group-choices label {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 1px solid var(--line);
    padding: 11px;
    border-radius: 8px;
    margin: 7px 0;
    background: var(--surface);
    cursor: pointer;
  }
  .consent-account:hover,
  .permission-choice:hover,
  .group-choices label:hover {
    background: var(--soft);
  }
  .chosen {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  input[type='checkbox'] {
    appearance: none;
    width: 16px;
    height: 16px;
    min-height: 0;
    padding: 0;
    margin: 0;
    border: 1px solid var(--muted);
    border-radius: 4px;
    background: var(--surface);
    display: grid;
    place-content: center;
    flex: none;
  }
  input[type='checkbox']:checked {
    background: var(--accent);
    border-color: var(--accent);
  }
  input[type='checkbox']:checked::after {
    content: '';
    width: 7px;
    height: 4px;
    border: solid white;
    border-width: 0 0 2px 2px;
    transform: rotate(-45deg);
    margin-top: -2px;
  }
  input[type='checkbox']:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }
  .identity,
  .permission-choice > span {
    min-width: 0;
    flex: 1;
  }
  strong {
    font-size: 12px;
    line-height: 1.4;
  }
  small {
    display: block;
    color: var(--muted);
    font-size: 11px;
    margin-top: 3px;
    overflow-wrap: anywhere;
    line-height: 1.45;
  }
  .identity strong {
    display: block;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .setup-note {
    display: flex;
    gap: 8px;
    align-items: start;
    padding: 11px;
    margin: 12px 0 0;
    border-radius: 8px;
    background: var(--soft);
    color: var(--muted);
    font-size: 11px;
    line-height: 1.55;
  }
  .duration-section label {
    margin-top: 10px;
  }
  .advanced-consent {
    padding: 12px;
    background: var(--soft);
    border: 1px solid var(--line);
    border-radius: 9px;
  }
  .request-identity {
    font-size: 11px;
    display: grid;
    gap: 8px;
    margin: 16px 0;
  }
  .request-identity dt {
    color: var(--muted);
    margin: 0 0 3px;
  }
  .request-identity dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
</style>
