<script lang="ts">
  import Icon from './Icon.svelte';
  export let action: 'export' | 'clear';
  export let label: string;
  export let confirm: () => Promise<string | void>;
  export let close: () => void;
  let busy = false;
  let error = '';
  let result = '';
  async function submit(): Promise<void> {
    busy = true;
    error = '';
    try {
      const receipt = await confirm();
      result = receipt ?? 'Histórico desta sessão removido.';
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="history-title">
    <div class="modal-title-row">
      <Icon name={action === 'export' ? 'folder-open' : 'trash'} size={23} />
      <h2 id="history-title">
        {action === 'export' ? 'Exportar' : 'Limpar'} histórico de {label}?
      </h2>
    </div>
    <p>
      {action === 'export'
        ? 'A exportação contém mensagens e eventos privados desta sessão. Não é um diagnóstico sanitizado; compartilhe somente com quem deve receber esse conteúdo.'
        : 'Isso remove mensagens e eventos locais somente desta sessão da aplicação. Não apaga contas, credenciais, projetos ou chats pessoais do Desktop.'}
    </p>
    <p class="form-hint">
      {action === 'export'
        ? 'A pasta será aberta. Após sete dias, esta cópia temporária será removida na próxima manutenção ou abertura do gerenciador; copie o ZIP se quiser guardá-lo.'
        : 'Esta ação é definitiva. Exporte antes se precisar preservar esse histórico.'}
    </p>
    {#if result}<p class="notice" role="status">{result}</p>{/if}
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}
    <div class="row modal-actions">
      <button class="button" disabled={busy} onclick={close}
        >{result ? 'Fechar' : 'Cancelar'}</button
      >{#if !result}<button
          class="button"
          class:danger={action === 'clear'}
          class:primary={action === 'export'}
          disabled={busy}
          onclick={submit}
          >{action === 'export' ? 'Exportar arquivo privado' : 'Limpar somente esta sessão'}</button
        >{/if}
    </div>
  </div>
</div>
