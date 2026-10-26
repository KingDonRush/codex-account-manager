<script lang="ts">
  import type { CreditBalance, QuotaPresentation, RateLimits, UsageWindow } from '../lib/types';
  import { planLabel } from '../lib/plan-label';
  import QuotaBar from './QuotaBar.svelte';
  import Icon from './Icon.svelte';
  export let limits: RateLimits | null;
  export let now: number;
  $: buckets = Object.entries(limits?.buckets ?? {});
  function quota(window: UsageWindow): QuotaPresentation {
    return {
      remaining:
        window.usedPercent === null ? null : Math.max(0, Math.min(100, 100 - window.usedPercent)),
      projected: false,
      windowDurationMins: window.windowDurationMins,
      resetsAt: window.resetsAt,
    };
  }
  function credits(value: CreditBalance): string {
    if (value.unlimited === true) return 'Créditos sem limite informado pelo servidor';
    if (value.balance !== null) return `Créditos disponíveis: ${value.balance}`;
    if (value.hasCredits === false) return 'Sem créditos disponíveis';
    return value.hasCredits ? 'Há créditos; saldo não informado' : 'Créditos não informados';
  }
</script>

{#if buckets.length || limits?.credits}
  <details class="detail-section quota-limits">
    <summary>Limites e créditos por recurso <Icon name="chevron-down" size={14} /></summary>
    {#if limits?.credits}<p class="form-hint">{credits(limits.credits)}</p>{/if}
    {#each buckets as [id, bucket] (id)}
      <section class="bucket" aria-label={`Limites de ${bucket.limitName ?? id}`}>
        <h4>{bucket.limitName ?? id}</h4>
        {#if bucket.planType}<p class="form-hint">
            Plano informado: {planLabel(bucket.planType)}
          </p>{/if}
        {#if bucket.primary}<QuotaBar window={quota(bucket.primary)} period="short" {now} />{/if}
        {#if bucket.secondary}<QuotaBar
            window={quota(bucket.secondary)}
            period="weekly"
            {now}
          />{/if}
        {#if bucket.credits}<p class="form-hint">{credits(bucket.credits)}</p>{/if}
        {#if bucket.rateLimitReachedType}<p class="form-hint">
            Restrição informada: {bucket.rateLimitReachedType}
          </p>{/if}
        {#if !bucket.primary && !bucket.secondary && !bucket.credits}<p class="form-hint">
            O servidor não publicou saldo para este recurso.
          </p>{/if}
      </section>
    {/each}
    <p class="form-hint">
      Os limites são os recebidos do servidor. Plano não garante acesso a todo modelo.
    </p>
  </details>
{/if}

<style>
  .bucket {
    padding: 12px 0;
    border-bottom: 1px solid var(--line);
  }
  .bucket h4 {
    margin: 0 0 10px;
  }
  .quota-limits {
    margin-top: 12px;
  }
</style>
