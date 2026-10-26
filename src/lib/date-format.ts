const accountDate = new Intl.DateTimeFormat('pt-BR', {
  day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
});

export function formatAccountDate(value: string | null): string {
  if (!value) return 'Ainda não registrado';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? 'Data indisponível' : accountDate.format(date);
}
