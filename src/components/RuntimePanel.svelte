<script lang="ts">
  import type { SharedRuntimeSnapshot, RuntimeRecoveryReport, RuntimeConsent } from '../lib/types';
  import Icon from './Icon.svelte';

  export let inspect: () => Promise<SharedRuntimeSnapshot>;
  export let recover: (thread: string, consent: RuntimeConsent | null) => Promise<RuntimeRecoveryReport>;
  export let open: (thread: string) => Promise<void>;
  let snapshot: SharedRuntimeSnapshot | null = null;
  let report: RuntimeRecoveryReport | null = null;
  let thread = '';
  let reportedThread = '';
  let busy = false;
  let error = '';
  let consent = false;

  async function inspectRuntime(): Promise<void> {
    if (busy) return;
    busy = true; error = '';
    try { snapshot = await inspect(); }
    catch (cause) { error = String(cause); }
    finally { busy = false; }
  }

  async function recoverChat(interrupt = false): Promise<void> {
    if (busy || !thread.trim()) return;
    busy = true; error = ''; consent = false;
    const target = thread.trim();
    try {
      report = await recover(target, interrupt ? report?.confirmation ?? null : null);
      reportedThread = target;
      snapshot = await inspect();
    } catch (cause) { error = String(cause); }
    finally { busy = false; }
  }

  async function openChat(): Promise<void> {
    try { await open(thread.trim()); }
    catch (cause) { error = String(cause); }
  }

  function reveal(event: Event): void {
    if ((event.currentTarget as HTMLDetailsElement).open && !snapshot) void inspectRuntime();
  }
</script>

<details class="maintenance-panel runtime-panel" ontoggle={reveal}>
  <summary>
    <span class="health-icon"><Icon name="activity" size={20} /></span>
    <span class="maintenance-copy"><strong>Runtime e chats bloqueados</strong><span>Verifique o proprietário e retome o chat original</span></span>
    <Icon name="chevron-down" size={16} />
  </summary>
  <div class="maintenance-content">
    <div class="maintenance-row">
      <div><strong>{snapshot?.state === 'busy' ? 'Servidor trabalhando' : snapshot?.state === 'idle' ? 'Servidor sem trabalho ativo' : snapshot?.state === 'absent' ? 'Nenhum daemon compartilhado' : 'Verificação de atividade'}</strong><p>{snapshot?.explanation ?? 'A consulta não retoma chats, lê mensagens ou troca sua conta.'}</p></div>
      <button class="button" disabled={busy} onclick={inspectRuntime}><Icon name="refresh" size={16} />{busy ? 'Verificando…' : 'Verificar novamente'}</button>
    </div>
    {#if snapshot && snapshot.threads.length > 0}
      <details class="runtime-threads"><summary>{snapshot.threads.length} chats no servidor{snapshot.pid ? ` · PID ${snapshot.pid}` : ''}</summary>
        <ul>{#each snapshot.threads as item (item.id)}<li>{item.name ?? item.id} · {item.state}{item.activeGoal ? ' · objetivo ativo' : ''}{item.backgroundTerminals ? ` · ${item.backgroundTerminals} terminais` : ''}</li>{/each}</ul>
      </details>
    {/if}
    {#if snapshot && snapshot.externalCalls.length > 0}<p>{snapshot.externalCalls.length} consumidores externos usando esta sessão · PIDs {snapshot.externalCalls.join(', ')}. Nenhum será encerrado pelo gerenciador.</p>{/if}
    {#if snapshot && !snapshot.externalInspectionSupported}<p>A detecção de CLI externa ainda não está disponível neste sistema. Integrações devem usar o lease documentado.</p>{/if}
    <form class="runtime-recovery" onsubmit={(event) => { event.preventDefault(); void recoverChat(); }}>
      <label for="runtime-thread">Chat que aparece como aberto em outro aplicativo</label>
      <input id="runtime-thread" placeholder="codex://threads/… ou UUID" bind:value={thread} disabled={busy} />
      <p>Se estiver seguro, o app libera o proprietário anterior e abre o mesmo chat. Não cria fork nem apaga histórico.</p>
      <div class="runtime-actions"><button class="button primary" type="submit" disabled={busy || !thread.trim()}><Icon name="shield-check" size={16} />{busy ? 'Verificando o proprietário…' : 'Recuperar chat original'}</button>
        <button class="button" type="button" disabled={busy || !thread.trim()} onclick={openChat}>Abrir chat original</button></div>
    </form>
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}
    {#if report && reportedThread === thread.trim()}
      <p role="status">{report.explanation}</p>
      {#if report.needsConfirmation}
        <div class="runtime-confirmation">
          <p>Encerrar esse servidor afeta <strong>{report.affectedThreads} chats</strong>, não apenas este. Aguarde o trabalho terminar e verifique novamente, ou autorize a interrupção.</p>
          <label><input type="checkbox" bind:checked={consent} disabled={busy} />Entendo que o trabalho em andamento pode ser interrompido.</label>
          <button class="button danger" disabled={!consent || busy} onclick={() => recoverChat(true)}>Interromper servidor e recuperar</button>
        </div>
      {/if}
    {/if}
  </div>
</details>

<style>
  .runtime-recovery { display: grid; gap: .65rem; margin: 1rem; }
  .runtime-recovery input { border: 1px solid var(--line); border-radius: 8px; padding: 10px; background: var(--surface); color: var(--ink); }
  .runtime-recovery p, .runtime-confirmation p { margin: 0; color: var(--muted); }
  .runtime-actions { display: flex; gap: .5rem; flex-wrap: wrap; }
  .runtime-confirmation { display: grid; gap: .8rem; margin: .75rem 1rem 1rem; }
  .runtime-confirmation label { display: flex; gap: .5rem; align-items: center; }
  .runtime-confirmation input { width: auto; }
  .runtime-confirmation button { justify-self: start; }
  .runtime-threads { margin: .8rem 0; overflow-wrap: anywhere; }
  .runtime-threads summary { cursor: pointer; }
  .runtime-threads li { margin: .3rem 0; }
</style>
