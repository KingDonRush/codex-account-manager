<script lang="ts">
  import Icon from './Icon.svelte';

  export let profileName: string;
  export let providerName: string;
  export let url: string;
  export let updateUrl: (value: string) => void;
  export let save: () => void;
  export let clear: () => void;
  export let close: () => void;
</script>

<div class="modal-backdrop" role="presentation">
  <div class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="mail-settings-title">
    <div class="modal-title-row">
      <span class="modal-icon"><Icon name="mail" size={22} /></span>
      <div>
        <h2 id="mail-settings-title">Webmail de {profileName}</h2>
        <p class="subtitle">Uma URL salva aqui sempre vence a sugestão do catálogo.</p>
      </div>
    </div>

    <div class="form">
      <label>
        Endereço da caixa de entrada
        <input
          aria-label="Endereço da caixa de entrada"
          value={url}
          oninput={(event) => updateUrl(event.currentTarget.value)}
          placeholder="https://webmail.exemplo.com"
        />
      </label>
      <p class="form-hint">Sugestão atual: {providerName}. O aplicativo apenas abre a página; ele não lê mensagens nem cookies.</p>
      <div class="row">
        <button class="button primary" disabled={!url.trim()} onclick={save}>
          <Icon name="circle-check" size={17} />Salvar endereço
        </button>
        {#if url}<button class="button" onclick={clear}><Icon name="trash" size={17} />Usar catálogo</button>{/if}
        <button class="button" onclick={close}>Cancelar</button>
      </div>
    </div>
  </div>
</div>
