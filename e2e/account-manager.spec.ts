import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Contas', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Cards', exact: true }).click();
});

test('shows remaining quota and one unambiguous primary action', async ({ page }) => {
  const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
  const filters = page.getByRole('region', { name: 'Filtrar contas' });

  await expect(filters).toContainText('3 contas');
  await expect(page.getByRole('tab', { name: /Conectadas 2/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(page.getByRole('tab', { name: /Atenção 1/ })).toBeVisible();
  await expect(card.getByText('100% restante')).toBeVisible();
  await expect(card.getByText('79% restante')).toBeVisible();
  await expect(card.getByText(/renova em|Reset passou/).first()).toBeVisible();
  await expect(page.getByText('Melhor próxima conta')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Em uso no Codex' })).toBeVisible();
  await expect(card.getByRole('button', { name: 'Reconectar' })).toHaveCount(0);
  await expect(card.getByRole('button', { name: 'Trocar' })).toHaveCount(0);
});

test('switches the active account with a clear state change', async ({ page }) => {
  const personal = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
  const work = page.locator('.account-card').filter({ hasText: 'Trabalho' });

  await work.getByRole('button', { name: 'Usar esta conta' }).click();
  await expect(
    page.getByText('Validando a credencial e reiniciando o Codex', { exact: true }),
  ).toBeVisible();
  await expect(page.getByText('Conta ativada no Codex')).toBeVisible();
  await page.getByRole('button', { name: 'Fechar' }).click();
  await expect(work.getByText('Em uso', { exact: true })).toBeVisible();
  await expect(work.getByRole('button', { name: 'Em uso no Codex' })).toBeVisible();
  await expect(personal.getByRole('button', { name: 'Usar esta conta' })).toBeVisible();

  await page.getByRole('button', { name: 'Diagnóstico' }).click();
  const operation = page.locator('.operation-item').filter({ hasText: 'Trabalho' }).first();
  await expect(operation.getByText('Tempos por estágio')).toHaveCount(0);
  await operation.locator('summary').click();
  await expect(operation.getByText('Tempos por estágio')).toBeVisible();
  await expect(operation.getByText('Espera pelo fechamento do Codex')).toBeVisible();
});

test('creates and renames an isolated profile from the secondary menu', async ({ page }) => {
  await page.getByRole('button', { name: 'Adicionar conta' }).click();
  await page.getByLabel('Nome').fill('Conta de testes');
  await page.getByText('Configurar e-mail agora').click();
  await page.getByLabel('Provedor de e-mail').selectOption('outlook');
  await page.getByRole('button', { name: 'Criar perfil' }).click();
  await page.getByRole('tab', { name: /Atenção/ }).click();

  const card = page.locator('.account-card').filter({ hasText: 'Conta de testes' });
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: 'Mais ações de Conta de testes' }).click();
  await page.getByRole('menuitem', { name: 'Renomear F2', exact: true }).click();
  await page.getByLabel('Novo nome').fill('Conta revisada');
  await page.getByRole('button', { name: 'Salvar' }).click();
  await expect(page.getByRole('heading', { name: 'Conta revisada' })).toBeVisible();
});

test('keeps reauthentication visible until the user cancels it', async ({ page }) => {
  const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
  await card.getByRole('button', { name: 'Mais ações de Conta pessoal' }).click();
  await page.getByRole('menuitem', { name: 'Reautenticar', exact: true }).click();

  await expect(page.getByText('Aguardando login no navegador')).toBeVisible();
  await page.getByRole('button', { name: 'Cancelar operação' }).click();
  await expect(page.getByText('Nenhuma credencial parcial foi salva.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Fechar' })).toBeVisible();
});

test('uses counters as filters and keeps secondary metadata disclosed', async ({ page }) => {
  await expect(page.getByText('Conta nova')).toHaveCount(0);
  await page.getByRole('tab', { name: /Atenção 1/ }).click();
  await expect(page.getByRole('heading', { name: 'Conta nova' })).toBeVisible();
  await page.getByRole('tab', { name: /Conectadas 2/ }).click();
  const card = page.locator('.account-card').filter({ hasText: 'Trabalho' });
  await expect(card.getByText('Último uso')).toBeVisible();
  await expect(card.getByText('Login no navegador')).toBeVisible();
  await expect(card.getByText('Quota observada')).not.toBeVisible();
  await card.getByText('Mais detalhes').click();
  await expect(card.getByText('Quota observada')).toBeVisible();
});

test('adds a custom webmail provider and exports sanitized diagnostics', async ({ page }) => {
  await page.getByRole('button', { name: 'E-mail', exact: true }).click();
  await page.getByRole('button', { name: 'Adicionar provedor' }).click();
  await page.getByLabel('Nome').fill('Webmail da empresa');
  await page.getByLabel('Domínios, separados por vírgula').fill('empresa.example');
  await page.getByLabel('URL do webmail').fill('https://mail.empresa.example');
  await page.getByRole('button', { name: 'Salvar' }).click();
  await expect(page.getByText('Webmail da empresa', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Diagnóstico' }).click();
  await page.getByText('Manutenção e exportação').click();
  await page.getByRole('button', { name: 'Exportar' }).click();
  await expect(page.getByText(/diagnostics-sanitized\.jsonl/)).toBeVisible();
  await page.getByRole('button', { name: 'Verificar atualizações' }).click();
  await expect(page.getByRole('button', { name: 'Instalar 0.4.1' })).toBeVisible();
});

test('saves and clears a per-account webmail override', async ({ page }) => {
  const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });

  await card.getByRole('button', { name: 'Mais ações de Conta pessoal' }).click();
  await page.getByRole('menuitem', { name: 'Configurar webmail', exact: true }).click();
  await page.getByLabel('Endereço da caixa de entrada').fill('https://mail.example.test/inbox');
  await page.getByRole('button', { name: 'Salvar endereço' }).click();
  await expect(page.getByText('Endereço de webmail salvo para esta conta.')).toBeVisible();

  await card.getByRole('button', { name: 'Mais ações de Conta pessoal' }).click();
  await page.getByRole('menuitem', { name: 'Configurar webmail', exact: true }).click();
  await expect(page.getByLabel('Endereço da caixa de entrada')).toHaveValue(
    'https://mail.example.test/inbox',
  );
  await page.getByRole('button', { name: 'Usar catálogo' }).click();
  await expect(page.getByText('A conta voltou a usar a sugestão do catálogo.')).toBeVisible();
});

test('keeps a dashboard with the current account volume responsive while scrolling', async ({
  page,
}) => {
  await page.goto('/?fixtureAccounts=14');
  await page.getByRole('button', { name: 'Cards', exact: true }).click();
  await expect(page.locator('.account-card')).toHaveCount(13);
  const metrics = await page.evaluate(async () => {
    const grid = document.querySelector<HTMLElement>('.accounts-grid');
    const scroller = document.querySelector<HTMLElement>('.content');
    if (!grid || !scroller) throw new Error('dashboard surface not found');
    const frameDurations: number[] = [];
    let previous = performance.now();
    for (let frame = 0; frame < 45; frame += 1) {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const current = performance.now();
      frameDurations.push(current - previous);
      previous = current;
      const progress = frame / 44;
      scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) * progress;
    }
    frameDurations.sort((left, right) => left - right);
    return {
      cards: grid.children.length,
      p95: frameDurations[Math.floor(frameDurations.length * 0.95)],
      maximum: frameDurations.at(-1) ?? 0,
    };
  });

  expect(metrics.cards).toBe(13);
  expect(metrics.p95).toBeLessThan(40);
  expect(metrics.maximum).toBeLessThan(100);
});

test('recalculates an expired window without switching account or reloading the page', async ({
  browser,
}) => {
  const page = await browser.newPage();
  await page.clock.install({ time: new Date('2026-10-05T15:00:00Z') });
  await page.goto('/');
  const card = page.locator('.account-card').filter({ hasText: 'Trabalho' });
  await page.getByRole('button', { name: 'Cards', exact: true }).click();
  await expect(card.getByText('26% restante')).toBeVisible();
  await page.clock.fastForward(6 * 60 * 1000);
  await expect(card.getByText('100% restante')).toBeVisible();
  await expect(card.getByText('Reset passou · aguardando confirmação')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Usar esta conta' })).toBeVisible();
  await page.close();
});

test('offers optional provider selection only after successful connection', async ({ page }) => {
  await page.goto('/?fixtureLogin=success');
  await page.getByRole('tab', { name: /Atenção 1/ }).click();
  await page.getByRole('button', { name: 'Conectar conta' }).click();
  await expect(page.getByText('Aguardando login no navegador')).toBeVisible();
  await expect(page.getByLabel('URL da caixa de e-mail')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Usar esta conta agora' })).toBeVisible();
  await page.getByLabel('Abrir e-mail junto numa próxima vez?').check();
  await page.getByLabel('Buscar provedor de e-mail').fill('Gmial');
  await expect(page.getByRole('button', { name: /Gmail \/ Google Workspace/ })).toBeVisible();
  await page.getByRole('button', { name: /Gmail \/ Google Workspace/ }).click();
  await page.getByRole('button', { name: 'Usar esta conta agora' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('tab', { name: /Conectadas 3/ })).toBeVisible();
});
