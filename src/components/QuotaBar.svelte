<script lang="ts">
  import type { QuotaPresentation } from '../lib/types';
  import { quotaPeriodLabel, quotaTone } from '../lib/view';
  import Icon from './Icon.svelte';

  export let window: QuotaPresentation;
  export let period: 'short' | 'weekly';
  export let now: number;

  $: projected = window.projected;
  $: remaining = window.remaining === null ? null : Math.round(window.remaining);
  $: tone = quotaTone(remaining);
  $: label = quotaPeriodLabel(window.windowDurationMins);
  $: reset = resetLabel(window.resetsAt, now);

  function resetLabel(timestamp: number | null, currentTime: number): string {
    if (timestamp === null) return '';
    const date = new Date(timestamp * 1000);
    if (Number.isNaN(date.getTime())) return '';
    const day = date.toLocaleDateString('pt-BR', { day: '2-digit', month: 'short' });
    const time = date.toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit' });
    if (timestamp * 1000 <= currentTime) {
      return projected ? 'Reset passou · aguardando confirmação' : 'Próximo reset será confirmado após o uso';
    }
    const minutes = Math.max(1, Math.ceil((timestamp * 1000 - currentTime) / 60_000));
    const relative = minutes >= 1440 ? `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h`
      : minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}min` : `${minutes}min`;
    return `renova em ${relative} · ${day}, ${time}`;
  }
</script>

<div class="quota-row">
  <div class="quota-label" title={reset || undefined}>
    <span class="quota-icon"><Icon name={period === 'short' ? 'clock' : 'calendar'} size={16} /></span>
    <span>{label}</span>
  </div>
  <strong>{remaining === null ? '—' : `${remaining}% restante`}</strong>
</div>
<div
  class:good={tone === 'good'}
  class:low={tone === 'low'}
  class:critical={tone === 'critical'}
  class:unknown={tone === 'unknown'}
  class="quota-track"
  role="progressbar"
  aria-label={`${label}: ${remaining === null ? 'saldo indisponível' : `${remaining}% restante`}`}
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={remaining ?? undefined}
>
  <span style:width={`${remaining ?? 0}%`}></span>
</div>
<div class:projected class="quota-reset">
  <Icon name="refresh" size={13} />
  <span>{reset || 'Horário de renovação indisponível'}{projected ? ' · estimativa' : ''}</span>
</div>
