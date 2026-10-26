<script lang="ts">
  import type {
    AccountCard,
    AccountFilter,
    DashboardCounts,
    ManagedProfile,
    ManagerSettings,
    ProfileOptions,
  } from '../lib/types';
  import Icon from './Icon.svelte';
  import ProfileCard from './ProfileCard.svelte';
  import AccountRow from './AccountRow.svelte';
  import AccountSpotlight from './AccountSpotlight.svelte';
  import AccountDetail from './AccountDetail.svelte';
  import AccountContextMenu from './AccountContextMenu.svelte';
  import type { AccountMenuAction } from '../lib/account-menu';
  import { profilePresentation } from '../lib/view';
  import { formatAccountDate } from '../lib/date-format';
  import emptyImage from '../assets/accounts-empty.png';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import { onMount } from 'svelte';
  import GroupFilter from './GroupFilter.svelte';
  import type { IntegrationSnapshot } from '../lib/integrations';
  import { groupAccounts } from '../lib/group-filter';

  export let accounts: AccountCard[];
  export let now: number;
  export let counts: DashboardCounts;
  export let providerName: (providerId: string | null) => string;
  export let connect: (profile: ManagedProfile, reconnect: boolean) => void;
  export let switchAccount: (profile: ManagedProfile) => void;
  export let openMail: (profile: ManagedProfile) => void;
  export let configureMail: (profile: ManagedProfile) => void;
  export let rename: (profile: ManagedProfile) => void;
  export let openFolder: (profile: ManagedProfile) => void;
  export let archive: (profile: ManagedProfile) => void;
  export let restore: (profile: ManagedProfile) => void;
  export let permanentlyRemove: (profile: ManagedProfile) => void;
  export let requestBatchDeletion: (profiles: ManagedProfile[]) => void;
  export let openBrowser: (profile: ManagedProfile) => void;
  export let manage: (profile: ManagedProfile) => void;
  export let check: (profile: ManagedProfile) => void;
  export let settings: ManagerSettings;
  export let saveSettings: (settings: ManagerSettings) => void;
  export let saveOptions: (profile: ManagedProfile, options: ProfileOptions) => void;
  export let integrations: IntegrationSnapshot;
  export let manageGroups: () => void;

  let filter: AccountFilter = 'connected';
  let search = '';
  let groupId = '';
  let layout: 'list' | 'grid' = 'list';
  let selectedId: string | null = null;
  let dismissedNotice = '';
  let batchMode = false;
  let batchIds = new Set<string>();
  function batchToggle(id: string, checked: boolean): void {
    const next = new Set(batchIds);
    if (checked) next.add(id);
    else next.delete(id);
    batchIds = next;
  }
  let menu: { profile: ManagedProfile; x: number; y: number; trigger: HTMLElement | null } | null =
    null;
  function context(event: MouseEvent, profile: ManagedProfile): void {
    event.preventDefault();
    const target = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
    const button =
      event.target instanceof Element ? event.target.closest<HTMLElement>('button') : null;
    menu = {
      profile,
      x: event.clientX,
      y: event.clientY,
      trigger: button ?? target?.querySelector<HTMLElement>('.menu-trigger') ?? null,
    };
  }
  function menuAction(action: AccountMenuAction, profile: ManagedProfile): void {
    const actions = {
      primary: activate,
      browser: openBrowser,
      mail: openMail,
      rename,
      check,
      archive: profile.status === 'archived' ? restore : archive,
      manage,
      reconnect: (profile: ManagedProfile) => connect(profile, true),
      configureMail,
      folder: openFolder,
      delete: permanentlyRemove,
    };
    actions[action](profile);
  }
  function keys(event: KeyboardEvent): void {
    if (menuTarget) return;
    if (
      document.querySelector('[aria-modal="true"]') ||
      (event.target instanceof HTMLElement &&
        event.target.closest('input,textarea,select,[contenteditable="true"]'))
    )
      return;
    const position = visibleAccounts.findIndex((item) => item.profile.id === selectedId);
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const next =
        visibleAccounts[
          Math.max(
            0,
            Math.min(visibleAccounts.length - 1, position + (event.key === 'ArrowDown' ? 1 : -1)),
          )
        ];
      if (next) selectedId = next.profile.id;
    }
    if (event.key === 'Enter' && selected) {
      event.preventDefault();
      activate(selected.profile);
    }
    if (event.key === 'F2' && selected) {
      event.preventDefault();
      rename(selected.profile);
    }
    if (event.key === 'F10' && event.shiftKey && selected) {
      event.preventDefault();
      menu = {
        profile: selected.profile,
        x: window.innerWidth / 2,
        y: 150,
        trigger: document.activeElement instanceof HTMLElement ? document.activeElement : null,
      };
    }
    if (event.key === 'Escape') {
      menu = null;
      selectedId = null;
    }
  }
  onMount(() => {
    document.addEventListener('keydown', keys);
    return () => document.removeEventListener('keydown', keys);
  });
  $: index = buildSearchIndex(accounts, (account) => [
    account.profile.name,
    account.profile.email ?? '',
  ]);
  $: matches = searchIndex(index, search);
  $: groupProfileIds = new Set(
    groupAccounts(integrations, groupId).map((account) => account.profileId),
  );
  $: scopedAccounts = accounts.filter(
    (account) => !groupId || groupProfileIds.has(account.profile.id),
  );
  $: scopedCounts = groupId
    ? {
        connected: scopedAccounts.filter((account) => account.health === 'connected').length,
        attention: scopedAccounts.filter((account) => account.health === 'attention').length,
        archived: scopedAccounts.filter((account) => account.health === 'archived').length,
      }
    : counts;
  $: visibleAccounts = matches.filter(
    (account) => account.health === filter && (!groupId || groupProfileIds.has(account.profile.id)),
  );
  $: if (groupId && !integrations.groups.some((group) => group.id === groupId && !group.archived))
    groupId = '';
  $: selected = accounts.find((account) => account.profile.id === selectedId);
  $: menuTarget = menu
    ? accounts.find((account) => account.profile.id === menu?.profile.id)?.profile
    : null;
  $: current = accounts.find((account) => account.profile.isActiveAccount);
  $: recommended = accounts.find((account) => account.recommendation);
  $: resets = accounts
    .filter((account) => account.health === 'connected' && account.nextResetAt)
    .sort((a, b) => Date.parse(a.nextResetAt!) - Date.parse(b.nextResetAt!))
    .slice(0, 5);
  $: lowBalanceKey =
    current &&
    !current.projectedReset &&
    current.effectiveRemaining !== null &&
    current.effectiveRemaining <= 15
      ? `${current.profile.id}-${Math.floor(current.effectiveRemaining / 5)}`
      : '';
  $: total = scopedCounts.connected + scopedCounts.attention + scopedCounts.archived;

  const filters: Array<{
    id: AccountFilter;
    label: string;
    icon: 'shield-check' | 'triangle-alert' | 'archive';
  }> = [
    { id: 'connected', label: 'Conectadas', icon: 'shield-check' },
    { id: 'attention', label: 'Atenção', icon: 'triangle-alert' },
    { id: 'archived', label: 'Arquivadas', icon: 'archive' },
  ];

  function select(profile: ManagedProfile): void {
    selectedId = profile.id;
  }
  function activate(profile: ManagedProfile): void {
    const action = profilePresentation(profile).action;
    if (action === 'connect') connect(profile, false);
    if (action === 'reauthenticate') connect(profile, true);
    if (action === 'switch') switchAccount(profile);
    if (action === 'check') check(profile);
    if (action === 'restore') restore(profile);
  }
</script>

{#if accounts.length === 0}
  <div class="empty-state">
    <img class="empty-art" src={emptyImage} alt="" />
    <h2>Nenhuma conta por aqui</h2>
    <p>Adicione um perfil e conecte sua primeira conta do Codex.</p>
  </div>
{:else}
  <div class="spotlight-grid">
    <AccountSpotlight account={current} current {activate} {select} /><AccountSpotlight
      account={recommended}
      {activate}
      {select}
    />
  </div>
  {#if lowBalanceKey && lowBalanceKey !== dismissedNotice}<div class="balance-notice" role="status">
      <Icon name="bell" size={16} /><span
        >A conta em uso tem {Math.round(current?.effectiveRemaining ?? 0)}% de saldo utilizável{recommended
          ? ` · ${recommended.profile.name} é a próxima recomendada.`
          : '.'}</span
      ><button
        class="menu-trigger"
        aria-label="Dispensar aviso de saldo"
        onclick={() => (dismissedNotice = lowBalanceKey)}><Icon name="circle-x" size={15} /></button
      >
    </div>{/if}
  <section class="account-filters" aria-label="Filtrar contas">
    <div class="filter-copy">
      <strong>{total} {total === 1 ? 'conta' : 'contas'}</strong>
    </div>
    <div class="filter-tabs" role="tablist" aria-label="Situação da conta">
      {#each filters as item (item.id)}
        <button
          class:active={filter === item.id}
          class:attention={item.id === 'attention' && scopedCounts[item.id] > 0}
          role="tab"
          aria-selected={filter === item.id}
          onclick={() => (filter = item.id)}
        >
          <Icon name={item.icon} size={16} />
          <span>{item.label}</span>
          <strong>{scopedCounts[item.id]}</strong>
        </button>
      {/each}
    </div>
  </section>

  <div class="accounts-toolbar">
    <GroupFilter
      groups={integrations.groups}
      bind:value={groupId}
      label="Filtrar contas por grupo"
      manage={manageGroups}
    />
    <button
      class="button"
      onclick={() => {
        batchMode = !batchMode;
        if (!batchMode) batchIds = new Set();
      }}>{batchMode ? 'Sair da seleção' : 'Selecionar contas'}</button
    >
    <label class="search-field"
      ><Icon name="search" size={17} /><input
        aria-label="Buscar contas"
        placeholder="Buscar por nome ou e-mail"
        bind:value={search}
      /></label
    >
    <div class="layout-toggle" role="group" aria-label="Visualização">
      <button
        aria-label="Lista compacta"
        aria-pressed={layout === 'list'}
        onclick={() => (layout = 'list')}><Icon name="list" size={17} /></button
      ><button aria-label="Cards" aria-pressed={layout === 'grid'} onclick={() => (layout = 'grid')}
        ><Icon name="grid" size={17} /></button
      >
    </div>
    <details class="rotation-settings">
      <summary><Icon name="settings" size={17} /><span>Rotação</span></summary>
      <div class="rotation-popover">
        <label
          >Priorizar<select
            value={settings.rotationStrategy}
            onchange={(event) =>
              saveSettings({
                rotationStrategy: event.currentTarget.value as ManagerSettings['rotationStrategy'],
              })}
            ><option value="before_reset">Saldo suficiente antes do reset</option><option
              value="largest_balance">Maior saldo disponível</option
            ></select
          ></label
        >
        <p>A recomendação não muda a conta ativa.</p>
      </div>
    </details>
  </div>

  {#if batchMode}<div class="selection-bar" role="region" aria-label="Ações da seleção">
      <span
        >{batchIds.size} {batchIds.size === 1 ? 'conta selecionada' : 'contas selecionadas'}</span
      >
      <button
        class="button small"
        onclick={() =>
          visibleAccounts
            .filter((account) => account.profile.accountId)
            .forEach((account) => batchToggle(account.profile.id, true))}
        >Selecionar resultados</button
      >
      {#if batchIds.size}<button class="button small" onclick={() => (batchIds = new Set())}
          >Limpar seleção</button
        >{/if}
      <button
        class="button small danger"
        disabled={!batchIds.size}
        onclick={() =>
          requestBatchDeletion(
            accounts
              .filter((account) => batchIds.has(account.profile.id))
              .map((account) => account.profile),
          )}><Icon name="trash" size={15} />Excluir seleção ({batchIds.size})</button
      >
    </div>{/if}

  <div class="accounts-workspace" class:with-detail={!!selected}>
    <div class="accounts-main">
      {#if visibleAccounts.length === 0}
        <div class="empty-state compact-empty">
          <span class="empty-icon"><Icon name="circle-check" size={25} /></span>
          <h2>Nada nesta lista</h2>
          <p>
            {filter === 'attention'
              ? 'Nenhuma conta precisa da sua atenção.'
              : 'Nenhuma conta corresponde a este filtro.'}
          </p>
        </div>
      {:else if layout === 'list'}
        <div class="accounts-list" aria-label="Lista de contas">
          {#each visibleAccounts as account (account.profile.id)}<AccountRow
              {account}
              {batchMode}
              batchSelected={batchIds.has(account.profile.id)}
              toggleBatch={(checked) => batchToggle(account.profile.id, checked)}
              selected={account.profile.id === selectedId}
              {select}
              {activate}
              menu={(event) => context(event, account.profile)}
            />{/each}
        </div>
      {:else}
        <div class="accounts-grid">
          {#each visibleAccounts as account (account.profile.id)}
            <ProfileCard
              {account}
              {now}
              providerName={providerName(account.profile.mailProviderId)}
              {connect}
              {switchAccount}
              {restore}
              {check}
              {batchMode}
              batchSelected={batchIds.has(account.profile.id)}
              toggleBatch={(checked) => batchToggle(account.profile.id, checked)}
              menu={(event) => context(event, account.profile)}
            />
          {/each}
        </div>
      {/if}
    </div>
    {#if selected}<AccountDetail
        account={selected}
        {now}
        {connect}
        {openMail}
        {configureMail}
        {rename}
        {openFolder}
        {openBrowser}
        {archive}
        {restore}
        {manage}
        {check}
        {saveOptions}
        close={() => (selectedId = null)}
      />{/if}
  </div>
  {#if resets.length}<details class="upcoming-resets">
      <summary
        ><Icon name="clock" size={16} />Próximos resets <Icon
          name="chevron-down"
          size={14}
        /></summary
      >
      <div>
        {#each resets as account (account.profile.id)}<button
            onclick={() => select(account.profile)}
            ><span>{account.profile.name}</span><time>{formatAccountDate(account.nextResetAt)}</time
            ></button
          >{/each}
      </div>
    </details>{/if}
{/if}
{#if menu && menuTarget}<AccountContextMenu
    profile={menuTarget}
    point={{ x: menu.x, y: menu.y }}
    trigger={menu.trigger}
    extended={layout === 'grid'}
    close={() => (menu = null)}
    select={(action) => menuTarget && menuAction(action, menuTarget)}
  />{/if}
