<script lang="ts">
  import type { Application } from '../lib/integrations';
  import { dialogFocus } from '../lib/dialog-focus';
  export let application: Application;
  export let save: (application: Application) => Promise<void>;
  export let close: () => void;
  let draft = structuredClone(application);
  let busy = false;
  let error = '';
  async function submit(): Promise<void> {
    busy = true;
    error = '';
    try {
      await save(draft);
      close();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
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
    aria-labelledby="application-title"
  >
    <h2 id="application-title">{draft.revision ? 'Editar aplicação' : 'Cadastrar aplicação'}</h2>
    <p class="subtitle">
      Cadastrar identifica a aplicação. O acesso a contas só será concedido quando você autorizar
      sua conexão.
    </p>
    <label>Nome<input bind:value={draft.name} maxlength="80" disabled={busy} /></label>
    <label
      >Identificador<input
        bind:value={draft.slug}
        placeholder="lead-studio"
        disabled={busy}
      /></label
    >
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}{#if busy}<p
        class="form-hint"
        role="status"
      >
        Salvando aplicação…
      </p>{/if}
    <div class="row modal-actions">
      <button
        class="button primary"
        disabled={busy || !draft.name.trim() || !draft.slug.trim()}
        onclick={submit}>Salvar</button
      ><button class="button" disabled={busy} onclick={close}>Cancelar</button>
    </div>
  </div>
</div>
