import { test, expect } from '@playwright/test';

test('a saved connection with pending browser cleanup retries only the owned closure', async ({ page }) => {
  await page.goto('/?fixtureLogin=success&fixtureBrowserCleanup=pending');
  await page.getByRole('tab', { name: /Atenção/ }).click();
  await page.locator('.account-row').filter({ hasText: 'Conta nova' }).getByRole('button', { name: 'Conectar conta', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('A conexão foi salva');
  await expect(dialog.getByRole('button', { name: 'Concluir fechamento do navegador', exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Concluir fechamento do navegador', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-mock-browser-cleanup-requests', '1');
  await expect(dialog.getByText('Conta conectada; navegador fechado', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Concluir fechamento do navegador', exact: true })).toHaveCount(0);
});

test('execution failure or cancellation keeps pending preservation visible and recovery never replays the job', async ({ page }, info) => {
  await page.setViewportSize({ width: 860, height: 620 });
  await page.goto('/?fixtureExecutions=pending');
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  const activity = page.getByRole('region', { name: 'Atividade de aplicações' });
  await activity.locator('summary').filter({ hasText: 'Trabalhos' }).click();
  const rows = activity.locator('.activity-row');
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText('Concluído · gravação no cofre pendente');
  await expect(rows.nth(1)).toContainText('Falhou · gravação no cofre pendente');
  await expect(rows.nth(2)).toContainText('Cancelado · gravação no cofre pendente');
  await expect(activity.getByRole('button', { name: /Reconectar|Repetir|Executar novamente/ })).toHaveCount(0);
  await rows.nth(2).getByRole('button', { name: 'Concluir gravação no cofre', exact: true }).click();
  await expect(rows.nth(2)).toContainText('Cancelado · houve pendência de sessão');
  await expect(rows.nth(2)).toBeVisible();
  await expect(activity.getByRole('button', { name: 'Concluir gravação no cofre', exact: true })).toHaveCount(0);
  await expect(rows).toHaveCount(3);
  await page.screenshot({ path: info.outputPath('execution-preservation-recovered.png') });
});

test('groups share accounts and editing survives UI navigation', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Gerenciar grupos', exact: true }).click();
  await page.getByRole('button', { name: 'Criar grupo', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Nome', { exact: true }).fill('Pesquisa');
  await dialog.getByRole('button', { name: 'Selecionar resultados', exact: true }).click();
  await dialog.getByRole('button', { name: 'Salvar', exact: true }).click();
  await expect(
    page.locator('.provider-item summary').filter({ hasText: 'Pesquisa' }),
  ).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Fechar', exact: true }).click();
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await page.getByRole('button', { name: 'Gerenciar grupos', exact: true }).click();
  await expect(
    page.locator('.provider-item summary').filter({ hasText: 'Pesquisa' }),
  ).toBeVisible();
});
test('approval grants account access and availability together without a binding setup step', async ({
  page,
}) => {
  await page.goto('/?fixtureConsent=pending');
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox', { name: 'Autorizar Trabalho', exact: true }).check();
  await expect(dialog.getByRole('checkbox', { name: /Executar com estas contas/ })).toBeChecked();
  await expect(dialog.getByText(/Habilita “Codex e aplicações”/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Autorizar e concluir acesso' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const resource = page.locator('.authorized-resource').filter({ hasText: 'Trabalho' });
  await expect(resource).toBeVisible();
  await expect(resource).toContainText('Login necessário');
  await expect(page.locator('.authorized-resource')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Adicionar vínculo' })).toHaveCount(0);
  await expect(resource.getByRole('button', { name: 'Conectar', exact: true })).toBeEnabled();
  await resource.getByRole('button', { name: 'Configurar conta' }).click();
  await expect(
    page.getByRole('dialog').getByRole('radio', { name: /Codex e aplicações/ }),
  ).toBeChecked();
  await expect(page.getByText('access_token', { exact: true })).toHaveCount(0);
});

test('a screen refresh failure after approval does not resubmit the consent', async ({ page }) => {
  await page.goto('/?fixtureConsent=pending&fixtureConsentRefresh=fail_once');
  let dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox', { name: 'Autorizar Trabalho', exact: true }).check();
  await dialog.getByRole('button', { name: 'Autorizar e concluir acesso' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Acesso já autorizado');
  await dialog.getByRole('button', { name: 'Abrir contas autorizadas' }).click();
  await expect(page.locator('.authorized-resource')).toHaveCount(1);
  await expect(page.locator('.application-card')).toHaveCount(1);
});

test('failure before launch keeps an actionable modal and does not propose OAuth for a coffer error', async ({
  page,
}) => {
  await page.goto('/?fixtureStartError=credential_unavailable');
  await page.getByRole('tab', { name: /Atenção/ }).click();
  await page.getByRole('button', { name: 'Conectar conta' }).click();
  const dialog = page.getByRole('dialog');
  await expect(
    dialog.getByText('A operação não chegou a começar. Nenhuma conta foi trocada.'),
  ).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Verificar acesso ao cofre' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Reconectar no navegador' })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Fechar' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('command palette tolerates typos and preserves the guarded account flow', async ({ page }) => {
  await page.goto('/');
  await page.keyboard.press('Control+k');
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Buscar comandos e contas').fill('trabaloh');
  await expect(dialog.getByRole('button', { name: /Trabalho/ })).toBeVisible();
  await dialog.getByLabel('Buscar comandos e contas').press('Enter');
  await expect(
    page.getByRole('dialog').getByRole('heading', { name: 'Usar Trabalho' }),
  ).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Fechar', exact: true }).click();
  await page.keyboard.press('Control+f');
  await expect(page.getByLabel('Buscar contas')).toBeFocused();
});

test('pending credential persistence offers recovery instead of another OAuth connection', async ({
  page,
}, info) => {
  await page.goto('/?fixtureRecovery=pending');
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await page.getByRole('button', { name: /Mostrar contas/ }).click();
  const row = page.locator('.resource-row').filter({ hasText: 'Gravação pendente' });
  await expect(row).toContainText('Trabalho');
  await expect(row.getByRole('button', { name: 'Reconectar', exact: true })).toHaveCount(0);
  await row.getByRole('button', { name: 'Configurar', exact: true }).click();
  await page
    .getByRole('dialog')
    .getByRole('button', { name: 'Concluir gravação no cofre' })
    .click();
  await expect(page.getByText('Gravação protegida pendente no cofre')).toHaveCount(0);
  await expect(
    page.getByRole('dialog').getByText('Sessão registrada', { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('dialog')).toContainText('A autenticação será validada ao executar');
  await expect(
    page.getByRole('dialog').getByRole('button', { name: 'Fechar', exact: true }),
  ).toBeInViewport();
  await page.screenshot({ path: info.outputPath('session-registered.png') });
});

test('history export is identified as private and clearing requires an explicit decision', async ({
  page,
}) => {
  await page.goto('/?fixtureHistory=closed');
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await page.getByRole('button', { name: 'Exportar histórico', exact: true }).click();
  let dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Não é um diagnóstico sanitizado');
  await dialog.getByRole('button', { name: 'Exportar arquivo privado' }).click();
  await expect(dialog.getByRole('status')).toHaveText('fixture-history.zip');
  await dialog.getByRole('button', { name: 'Fechar', exact: true }).click();
  await page.getByRole('button', { name: 'Limpar histórico', exact: true }).click();
  dialog = page.getByRole('dialog');
  await expect(dialog).toContainText(
    'Não apaga contas, credenciais, projetos ou chats pessoais do Desktop',
  );
  await dialog.getByRole('button', { name: 'Limpar somente esta sessão' }).click();
  await expect(dialog.getByRole('status')).toHaveText('Histórico desta sessão removido.');
});

for (const height of [620, 790]) {
  test(`session settings retain a visible close action at height ${height}`, async ({ page }) => {
    await page.setViewportSize({ width: 860, height });
    await page.goto('/?fixtureRecovery=pending');
    await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
    await page.getByRole('button', { name: /Mostrar contas/ }).click();
    await page
      .locator('.resource-row')
      .filter({ hasText: 'Gravação pendente' })
      .getByRole('button', { name: 'Configurar', exact: true })
      .click();
    const dialog = page.getByRole('dialog');
    const close = dialog.getByRole('button', { name: 'Fechar', exact: true });
    await expect(close).toBeInViewport({ ratio: 1 });
    await dialog.getByRole('button', { name: 'Concluir gravação no cofre' }).click();
    await expect(dialog.getByText('Sessão registrada', { exact: true })).toBeVisible();
    await expect(close).toBeInViewport({ ratio: 1 });
    await close.click();
    await expect(dialog).toHaveCount(0);
  });
}
