<script lang="ts">
  import type { IntegrationAccount } from '../lib/integrations';
  import Icon from './Icon.svelte';
  export let account: IntegrationAccount;
  export let confirm: () => Promise<void>;
  export let close: () => void;
  let busy = false;
  let error = '';
  async function share(): Promise<void> {
    busy = true;
    try {
      await confirm();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="share-title">
    <div class="modal-title-row">
      <Icon name="shield-check" size={24} />
      <h2 id="share-title">Compartilhar {account.name} com o Desktop?</h2>
    </div>
    <p>
      Esta conta está reservada para aplicações. Usá-la no Codex muda a política para compartilhada;
      os aplicativos autorizados continuam com suas sessões próprias.
    </p>
    <p class="form-hint">A sessão do Desktop não será copiada para os aplicativos.</p>
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}
    <div class="row modal-actions">
      <button class="button" disabled={busy} onclick={close}>Cancelar</button><button
        class="button primary"
        disabled={busy}
        onclick={share}>Compartilhar e usar</button
      >
    </div>
  </div>
</div>
