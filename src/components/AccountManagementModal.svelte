<script lang="ts">
  import type { ManagedProfile } from '../lib/types';
  import { api } from '../lib/api';
  import Icon from './Icon.svelte';
  import PlanBadge from './PlanBadge.svelte';
  import AccountUsageSettings from './AccountUsageSettings.svelte';
  import { integrationApi, type IntegrationAccount, type AccountPolicy } from '../lib/integrations';
  import { dialogFocus } from '../lib/dialog-focus';
  import { applicationSessionPresentation } from '../lib/application-access';
  export let profile: ManagedProfile;
  export let profiles: ManagedProfile[];
  export let replace: () => void;
  export let switchAccount: (profile: ManagedProfile) => void;
  export let changed: () => Promise<void>;
  export let close: () => void;
  export let initiallyDelete = false;
  export let integrationAccount: IntegrationAccount | null = null;
  export let connectSession: (profileId: string) => void;
  let policy: AccountPolicy = integrationAccount?.policy ?? 'desktop_only';
  let capacity = integrationAccount?.capacity ?? 1;
  let feedback = '';
  $: dirty =
    integrationAccount &&
    (policy !== integrationAccount.policy || capacity !== integrationAccount.capacity);
  let mode: 'choose' | 'unlink' | 'delete' = initiallyDelete ? 'delete' : 'choose';
  let typedName = '';
  let understood = false;
  let deleteBrowser = true;
  let busy = false;
  let error = '';
  let activeCleared = false;
  let otherId = '';
  let removalOpen = false;
  $: active = profile.isActiveAccount && !activeCleared;
  $: applicationSession = integrationAccount
    ? applicationSessionPresentation(integrationAccount)
    : null;
  $: alternatives = profiles.filter(
    (item) =>
      item.id !== profile.id &&
      item.accountId &&
      item.status === 'active' &&
      item.credentialState === 'ready',
  );
  $: confirmed = typedName === profile.name && understood && !active;

  async function logout(): Promise<void> {
    busy = true;
    error = '';
    try {
      await api.logoutProfile(profile.id);
      activeCleared = true;
      await changed();
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function submit(): Promise<void> {
    if (!confirmed || busy) return;
    busy = true;
    error = '';
    try {
      if (mode === 'unlink' || !deleteBrowser) await api.unlinkProfile(profile.id);
      else await api.deleteProfile(profile.id, true);
      await changed();
      close();
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  function useAlternative(): void {
    const other = alternatives.find((item) => item.id === otherId);
    if (other) {
      close();
      switchAccount(other);
    }
  }

  async function saveUsage(): Promise<void> {
    if (!integrationAccount || busy) return;
    busy = true;
    error = '';
    feedback = '';
    try {
      await integrationApi.accountPolicy(integrationAccount.id, policy, capacity);
      await changed();
      feedback = 'Uso da conta salvo. A conta ativa do Codex não foi trocada.';
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }
  async function prepareApplicationSession(): Promise<void> {
    if (applicationSession?.kind === 'recovery') {
      busy = true;
      error = '';
      try {
        await integrationApi.recoverCredential(profile.id);
        await changed();
        feedback = 'Gravação no cofre concluída.';
      } catch (cause) {
        error = String(cause);
      } finally {
        busy = false;
      }
      return;
    }
    close();
    connectSession(profile.id);
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal compact-modal"
    tabindex="-1"
    use:dialogFocus={{
      escape: () => {
        if (!busy) close();
      },
    }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="management-title"
  >
    <div class="modal-title-row">
      <span class="modal-icon"><Icon name="settings" size={22} /></span>
      <div>
        <h2 id="management-title">{profile.name}</h2>
        <p class="subtitle">{profile.email ?? 'Perfil sem conta vinculada'}</p>
        <PlanBadge plan={profile.planType} />
      </div>
    </div>
    {#if error}<div class="notice error" role="alert">{error}</div>{/if}
    {#if feedback}<p class="notice" role="status">{feedback}</p>{/if}
    {#if mode === 'choose' && integrationAccount}<section class="management-usage">
        <AccountUsageSettings bind:policy bind:capacity disabled={busy} />
        <button class="button primary" disabled={busy || !dirty} onclick={saveUsage}
          >Salvar uso da conta</button
        >
        {#if integrationAccount.policy !== 'desktop_only' && applicationSession}<div
            class="management-session"
          >
            <strong>Sessão para aplicações</strong>
            <p class="form-hint">
              {applicationSession.label}. {applicationSession.guidance}
            </p>
            {#if applicationSession.actionLabel}<button
                class="button small"
                disabled={busy}
                onclick={prepareApplicationSession}
                ><Icon name="link" size={16} />{applicationSession.actionLabel}</button
              >{/if}
          </div>{/if}
      </section>{/if}
    {#if active && (mode !== 'choose' || removalOpen)}<div class="active-account-guard">
        <p>
          Esta conta está em uso no Codex. Antes de substituir, desvincular ou excluir, use outra
          conta ou saia da sessão local.
        </p>
        {#if alternatives.length}<div class="row form">
            <select aria-label="Outra conta para usar" bind:value={otherId}
              ><option value="">Escolher outra conta</option
              >{#each alternatives as other (other.id)}<option value={other.id}>{other.name}</option
                >{/each}</select
            ><button class="button small" disabled={!otherId || busy} onclick={useAlternative}
              >Usar outra</button
            >
          </div>{/if}
        <details>
          <summary>Sair sem escolher outra conta</summary>
          <p>Fecha o Codex e o reabre sem sessão. O histórico e a credencial salva são mantidos.</p>
          <button class="button small" disabled={busy} onclick={logout}
            ><Icon name="log-out" size={16} />{busy ? 'Saindo…' : 'Sair do Codex'}</button
          >
        </details>
      </div>{/if}
    {#if mode === 'choose'}<details class="management-removal" bind:open={removalOpen}>
        <summary><Icon name="settings" size={16} />Associação e remoção de dados</summary>
        <div class="management-options">
          {#if profile.accountId}<button
              disabled={active || busy || profile.status !== 'active'}
              onclick={replace}
              ><Icon name="replace" size={19} /><span
                ><strong>Substituir conta vinculada</strong><small
                  >Faça login na outra conta e confirme depois. Mantém o perfil e remove a sessão
                  salva antiga após concluir.</small
                ></span
              ></button
            ><button disabled={active || busy} onclick={() => (mode = 'unlink')}
              ><Icon name="unlink" size={19} /><span
                ><strong>Desvincular conta</strong><small
                  >Remove a sessão salva deste app e mantém o navegador.</small
                ></span
              ></button
            >{/if}
          <button
            disabled={active || busy}
            class="management-danger"
            onclick={() => (mode = 'delete')}
            ><Icon name="trash" size={19} /><span
              ><strong>Excluir definitivamente</strong><small
                >Escolha quais dados locais remover.</small
              ></span
            ></button
          >
        </div>
      </details>
      <div class="row modal-actions">
        <button class="button" disabled={busy} onclick={close}>Fechar</button>
      </div>
    {:else}<form
        class="form"
        onsubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <h3>{mode === 'delete' ? 'Excluir dados locais' : 'Desvincular conta'}</h3>
        <p class="subtitle">
          {mode === 'delete'
            ? 'A associação e a credencial salva serão removidas.'
            : 'O nome e os cookies do perfil ficam; ele poderá receber outra conta depois.'} Não exclui
          nem revoga a conta remota. Não remove projetos ou histórico do Codex.
        </p>
        {#if mode === 'delete'}<label class="check-label"
            ><input type="checkbox" bind:checked={deleteBrowser} />Também excluir os dados do
            navegador deste perfil</label
          >{/if}
        <label
          >Digite “{profile.name}” para confirmar<input
            aria-label="Nome do perfil para confirmar"
            bind:value={typedName}
            autocomplete="off"
          /></label
        >
        <label class="check-label"
          ><input type="checkbox" bind:checked={understood} />Entendi quais dados locais serão
          removidos</label
        >
        <div class="row modal-actions">
          <button class="button danger" type="submit" disabled={!confirmed || busy}
            ><Icon name="trash" size={16} />{busy
              ? 'Removendo…'
              : mode === 'delete'
                ? 'Excluir dados locais'
                : 'Desvincular conta'}</button
          ><button class="button" type="button" disabled={busy} onclick={close}>Cancelar</button>
        </div>
      </form>{/if}
  </div>
</div>

<style>
  .management-usage {
    margin: 20px 0;
  }
  .management-usage > .button {
    margin-top: 14px;
  }
  .management-session {
    background: var(--soft);
    border-radius: 9px;
    padding: 14px;
    margin-top: 16px;
  }
  .management-session strong {
    font-size: 12px;
  }
  .management-removal {
    border-top: 1px solid var(--line);
    padding-top: 14px;
    margin-top: 18px;
  }
  .management-removal summary {
    display: flex;
    align-items: center;
    gap: 7px;
    cursor: pointer;
    color: var(--muted);
    font-size: 12px;
  }
  .modal-title-row > div {
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
