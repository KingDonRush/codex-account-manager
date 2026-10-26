<script lang="ts">
  import { onMount } from 'svelte';
  import {
    integrationApi,
    type IntegrationSnapshot,
    type ExecutionStatus,
    type ApplicationServerStatus,
  } from '../lib/integrations';
  import Icon from './Icon.svelte';
  import SessionHistoryModal from './SessionHistoryModal.svelte';
  import { executionOutcomeLabel } from '../lib/application-access';
  export let snapshot: IntegrationSnapshot;
  export let changed: () => Promise<void>;
  let jobs: ExecutionStatus[] = [];
  let servers: ApplicationServerStatus[] = [];
  let loading = false;
  let busyId = '';
  let disposed = false;
  let error = '';
  let history: { id: string; label: string; action: 'export' | 'clear' } | null = null;
  let jobsExpanded = false;
  onMount(() => {
    void load();
    const timer = window.setInterval(() => void load(), 2000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  });
  async function load(): Promise<void> {
    if (loading || disposed) return;
    loading = true;
    try {
      const [nextJobs, nextServers] = await Promise.all([
        integrationApi.executions(),
        integrationApi.applicationServers(),
      ]);
      if (!disposed) {
        if (
          nextJobs.some(
            (job) =>
              activeStates.includes(job.state) && !jobs.some((previous) => previous.id === job.id),
          )
        ) {
          jobsExpanded = true;
        }
        jobs = nextJobs;
        servers = nextServers;
      }
    } catch (cause) {
      if (!disposed) error = String(cause);
    } finally {
      loading = false;
    }
  }
  async function act(id: string, action: () => Promise<void>): Promise<void> {
    busyId = id;
    error = '';
    try {
      await action();
      await load();
    } catch (cause) {
      error = String(cause);
    } finally {
      busyId = '';
    }
  }
  async function confirmHistory(): Promise<string | void> {
    if (!history) return;
    const result =
      history.action === 'clear'
        ? await integrationApi.clearHistory(history.id)
        : await integrationApi.exportHistory(history.id);
    await load();
    return result;
  }
  function applicationName(id: string): string {
    return snapshot.applications.find((app) => app.id === id)?.name ?? 'Aplicação';
  }
  function accountName(id: string | null): string {
    return snapshot.accounts.find((account) => account.id === id)?.name ?? 'Aguardando conta';
  }
  const activeStates = ['queued', 'preparing', 'running', 'cancelling'];
</script>

{#if error}<p class="notice error" role="alert">
    Não foi possível atualizar a atividade: {error}
  </p>{/if}
{#if jobs.length || servers.length}<section
    class="application-activity"
    aria-label="Atividade de aplicações"
  >
    {#if jobs.length}<details class="form-disclosure" bind:open={jobsExpanded}>
        <summary
          ><Icon name="activity" size={16} />Trabalhos
          <span class="count">{jobs.length}</span></summary
        >
        {#each jobs as job (job.id)}
          {@const account = snapshot.accounts.find((account) => account.id === job.accountId)}
          <div class="activity-row">
            <div>
              <strong>{applicationName(job.applicationId)}</strong><small
                >{executionOutcomeLabel(job, account)} · {accountName(job.accountId)}</small
              >
            </div>
            {#if activeStates.includes(job.state)}<button
                class="button small danger"
                disabled={busyId === job.id}
                onclick={() => act(job.id, () => integrationApi.cancelExecution(job.id))}
                >Cancelar</button
              >{:else if account && account.applicationSessionRecoveryPending && job.errorCode === 'credential_recovery_pending'}
              <button
                class="button small"
                disabled={busyId === job.id}
                onclick={() =>
                  act(job.id, async () => {
                    await integrationApi.recoverCredential(account.profileId);
                    await changed();
                  })}><Icon name="shield-check" size={15} />Concluir gravação no cofre</button
              >
            {/if}
          </div>{/each}
      </details>{/if}
    {#if servers.length}<details class="form-disclosure" open>
        <summary
          ><Icon name="terminal" size={16} />Sessões App Server
          <span class="count">{servers.length}</span></summary
        >
        {#each servers as session (session.id)}<div class="activity-row">
            <div>
              <strong>{applicationName(session.applicationId)}</strong><small
                >{accountName(session.accountId)} · {session.state}{session.errorCode
                  ? ` · ${session.errorCode}`
                  : ''}</small
              >
            </div>
            <div class="row">
              {#if !['closed', 'failed', 'interrupted'].includes(session.state)}<button
                  class="button small danger"
                  disabled={busyId === session.id}
                  onclick={() =>
                    act(session.id, () => integrationApi.closeApplicationServer(session.id))}
                  >Encerrar sessão</button
                >
              {:else}<button
                  class="button small"
                  onclick={() =>
                    (history = {
                      id: session.id,
                      label: applicationName(session.applicationId),
                      action: 'export',
                    })}>Exportar histórico</button
                ><button
                  class="button small danger"
                  onclick={() =>
                    (history = {
                      id: session.id,
                      label: applicationName(session.applicationId),
                      action: 'clear',
                    })}>Limpar histórico</button
                >{/if}
            </div>
          </div>{/each}
      </details>{/if}
  </section>{/if}
{#if history}<SessionHistoryModal
    action={history.action}
    label={history.label}
    confirm={confirmHistory}
    close={() => (history = null)}
  />{/if}

<style>
  .application-activity {
    padding: 0 20px;
    margin-top: 20px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
  }
  .activity-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 12px 0;
    border-top: 1px solid var(--line);
  }
  .activity-row > div:first-child {
    min-width: 0;
  }
  strong {
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  small {
    display: block;
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
  }
  .count {
    color: var(--muted);
    margin-left: auto;
  }
  @media (max-width: 740px) {
    .activity-row {
      flex-wrap: wrap;
    }
  }
</style>
