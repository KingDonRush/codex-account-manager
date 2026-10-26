<script lang="ts">
  import { onMount } from 'svelte';
  import type { OperationStage } from '../lib/types';
  import { operationStageLabel, stageDurationLabel } from '../lib/operation-stage-view';
  import Icon from './Icon.svelte';

  export let operationId: string;
  export let load: (operationId: string) => Promise<OperationStage[]>;

  let stages: OperationStage[] = [];
  let loading = true;
  let error = '';

  onMount(() => {
    void loadStages();
  });

  async function loadStages(): Promise<void> {
    try {
      stages = await load(operationId);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }
</script>

<section class="operation-timeline" aria-label="Tempos por estágio">
  <div class="timeline-heading">
    <Icon name="activity" size={15} />
    <strong>Tempos por estágio</strong>
  </div>
  {#if loading}
    <p>Carregando detalhamento…</p>
  {:else if error}
    <p class="timeline-error">Não foi possível carregar: {error}</p>
  {:else if stages.length === 0}
    <p>Esta operação foi gravada por uma versão sem detalhamento por estágio.</p>
  {:else}
    <ol>
      {#each stages as stage, index (`${stage.name}-${index}`)}
        <li>
          <span>{operationStageLabel(stage.name)}</span>
          <strong>{stageDurationLabel(stage.durationMs)}</strong>
        </li>
      {/each}
    </ol>
  {/if}
</section>
