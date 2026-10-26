<script lang="ts">
  import { planLabel } from '../lib/plan-label';
  export let plan: string | null | undefined;
</script>

<span class="plan-badge" class:unknown={!plan} aria-label={`Plano ${planLabel(plan)}`}
  >{plan ? planLabel(plan) : 'Plano não informado'}</span
>

<style>
  .plan-badge {
    display: inline-flex;
    flex: none;
    align-items: center;
    width: fit-content;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 2px 6px;
    color: var(--ink);
    background: var(--soft);
    font-size: 10px;
    font-weight: 600;
    line-height: 1.5;
    white-space: nowrap;
  }
  .plan-badge.unknown {
    color: var(--muted);
    font-weight: 400;
  }
</style>
