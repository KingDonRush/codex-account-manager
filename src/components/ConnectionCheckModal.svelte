<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import type { ManagedProfile, ConnectionCheck } from '../lib/types';
  import recoveryImage from '../assets/session-recovery.png';
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';
  export let profile: ManagedProfile;
  export let changed: () => Promise<void>;
  export let close: () => void;
  export let activate: (profile: ManagedProfile) => void;
  let result: ConnectionCheck | null = null;
  let busy = true;
  let error = '';
  async function check(): Promise<void> {
    busy = true;
    error = '';
    result = null;
    try {
      result = await api.checkProfile(profile.id);
      await changed();
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    void check();
  });
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal compact-modal"
    tabindex="-1"
    use:dialogFocus={{ escape: close }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="check-title"
  >
    <div class="check-title">
      <img src={recoveryImage} alt="" />
      <div>
        <h2 id="check-title">Verificar {profile.name}</h2>
        <p class="subtitle">Sem trocar a conta ativa</p>
      </div>
    </div>
    {#if busy}<div class="check-progress" role="status">
        <span class="activity-ring" aria-hidden="true"></span>Consultando sessão e saldo…
      </div>
    {:else if result}<h3>{result.title}</h3>
      <p class="subtitle">{result.guidance}</p>
      {#if result.errorCode}<details class="detail-section">
          <summary>Detalhes técnicos <Icon name="chevron-down" size={14} /></summary>
          <p class="form-hint">Código: {result.errorCode}</p>
        </details>{/if}
    {:else if error}<div class="notice error" role="alert">{error}</div>{/if}
    {#if !busy && result?.canRenew}<p class="form-hint">
        A próxima ação valida e, se possível, renova a sessão salva antes de trocar a conta no
        Codex. Você acompanhará a preparação e o resultado.
      </p>{/if}
    <div class="row login-actions">
      {#if !busy && result?.canRenew}<button
          class="button primary"
          onclick={() => activate(profile)}
          ><Icon name="key-refresh" size={16} />Renovar e usar no Codex</button
        >{/if}{#if !busy && !result?.confirmed}<button class="button small" onclick={check}
          ><Icon name="retry" size={16} />Verificar novamente</button
        >{/if}<button class="button" onclick={close}>Fechar</button>
    </div>
  </div>
</div>
