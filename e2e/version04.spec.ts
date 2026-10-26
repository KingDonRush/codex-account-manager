import { expect, test, type Page } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/');
});

async function detail(page: Page, name = 'Trabalho'): Promise<void> {
  await page.getByRole('button', { name: `Detalhes de ${name}`, exact: true }).click();
}

async function manage(page: Page): Promise<void> {
  await detail(page);
  await page.getByText('Gerenciar perfil', { exact: true }).click();
  await page.getByRole('button', { name: 'Gerenciar conta', exact: true }).click();
  await page
    .getByRole('dialog')
    .getByText('Associação e remoção de dados', { exact: true })
    .click();
}

test('starts compact and reveals quota, reset, history and controls on demand', async ({
  page,
}) => {
  await expect(page.locator('.account-row')).toHaveCount(2);
  await expect(page.locator('.account-card')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
  await expect(page.getByRole('region', { name: 'Próxima conta recomendada' })).toContainText(
    'Trabalho',
  );
  await expect(page.getByText('Login confirmado', { exact: true })).toHaveCount(0);
  await detail(page);
  const panel = page.getByRole('complementary', { name: 'Detalhes de Trabalho' });
  await expect(panel.getByText('26% restante')).toBeVisible();
  await expect(panel.getByText(/renova em 5min/)).toBeVisible();
  await expect(panel.getByText('Login confirmado', { exact: true })).not.toBeVisible();
  await panel.getByText('Histórico e conexão', { exact: true }).click();
  await expect(panel.getByText('Login confirmado', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Fechar detalhes' }).click();
  await page.getByLabel('Buscar contas').fill('equipe@gmail');
  await expect(page.locator('.account-row')).toHaveCount(1);
  await expect(page.locator('.account-row')).toContainText('Trabalho');
});

test('persists light and dark choices and follows the system in automatic mode', async ({
  page,
}) => {
  await page.getByRole('button', { name: 'Tema escuro' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.getByRole('button', { name: 'Tema claro' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.getByRole('button', { name: 'Tema sistema' }).click();
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('can pause recommendation without archiving the account', async ({ page }) => {
  await detail(page);
  const panel = page.getByRole('complementary', { name: 'Detalhes de Trabalho' });
  await panel.getByText('Rotação', { exact: true }).click();
  await panel.getByLabel('Incluir na recomendação').uncheck();
  await expect(page.locator('.account-row')).toHaveCount(2);
  await expect(page.getByRole('region', { name: 'Próxima conta recomendada' })).toContainText(
    'Nenhuma outra conta elegível',
  );
  await panel.getByLabel('Incluir na recomendação').check();
  await panel.getByLabel('Preferir quando houver saldo').check();
  await expect(panel.getByLabel('Preferir quando houver saldo')).toBeChecked();
});

test('replacement preserves the old association until explicit confirmation', async ({ page }) => {
  await manage(page);
  await page.getByRole('button', { name: 'Substituir conta vinculada' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText('Confirme a substituição da conta', { exact: true })).toBeVisible();
  await expect(dialog.getByText('equipe@gmail.com', { exact: true })).toBeVisible();
  await expect(page.locator('.account-row').filter({ hasText: 'Trabalho' })).toContainText(
    'equipe@gmail.com',
  );
  await dialog.getByRole('button', { name: 'Confirmar substituição' }).click();
  await expect(dialog.getByText('Conta substituída', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Fechar' }).click();
  await expect(page.locator('.account-row').filter({ hasText: 'Trabalho' })).toContainText(
    'nova.conta@proton.me',
  );
});

test('cancelled replacement retains the original account and profile', async ({ page }) => {
  await manage(page);
  await page.getByRole('button', { name: 'Substituir conta vinculada' }).click();
  await expect(page.getByRole('button', { name: 'Confirmar substituição' })).toBeVisible();
  await page.getByRole('button', { name: 'Cancelar operação' }).click();
  await expect(
    page.getByRole('dialog').getByText('Nenhuma credencial parcial foi salva.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Fechar', exact: true }).click();
  await expect(page.locator('.account-row').filter({ hasText: 'Trabalho' })).toContainText(
    'equipe@gmail.com',
  );
});

test('deletes an active-list profile only after two meaningful confirmations', async ({ page }) => {
  await manage(page);
  await page.getByRole('button', { name: 'Excluir definitivamente' }).click();
  const dialog = page.getByRole('dialog');
  const submit = dialog.getByRole('button', { name: 'Excluir dados locais', exact: true });
  await expect(submit).toBeDisabled();
  await dialog.getByLabel('Nome do perfil para confirmar').fill('Conta pessoal');
  await dialog.getByLabel('Entendi quais dados locais serão removidos').check();
  await expect(submit).toBeDisabled();
  await dialog.getByLabel('Nome do perfil para confirmar').fill('Trabalho');
  await submit.click();
  await expect(page.locator('.account-row')).toHaveCount(1);
  await expect(page.locator('.account-row')).toContainText('Conta pessoal');
});

test('unlink keeps the named browser profile available to connect again', async ({ page }) => {
  await manage(page);
  await page.getByRole('button', { name: 'Desvincular conta', exact: false }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Nome do perfil para confirmar').fill('Trabalho');
  await dialog.getByLabel('Entendi quais dados locais serão removidos').check();
  await dialog.getByRole('button', { name: 'Desvincular conta', exact: true }).click();
  await page.getByRole('tab', { name: /Atenção/ }).click();
  const row = page.locator('.account-row').filter({ hasText: 'Trabalho' });
  await expect(row).toContainText('Conecte uma conta');
  await expect(row.getByRole('button', { name: 'Conectar conta' })).toBeVisible();
});

test('checks another account without switching the active identity', async ({ page }) => {
  await detail(page);
  await page.getByText('Histórico e conexão', { exact: true }).click();
  await page.getByRole('button', { name: 'Verificar conexão e saldo' }).click();
  await expect(
    page.getByRole('dialog').getByRole('heading', { name: 'Conexão e saldo confirmados' }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Fechar', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
});

test('a changed query context is disclosed without claiming connection, renewing, or switching', async ({ page }, info) => {
  await page.setViewportSize({ width: 860, height: 620 });
  await page.goto('/?fixtureCheck=changed');
  const row = page.locator('.account-row').filter({ hasText: 'Trabalho' });
  await row.getByRole('button', { name: /Mais ações/ }).click();
  await page.getByRole('menuitem', { name: 'Verificar conexão', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading', { name: 'A conta ou sessão mudou', exact: true })).toBeVisible();
  await expect(dialog.getByText(/nenhuma conta diferente foi confirmada/)).toBeVisible();
  await expect(dialog.getByRole('button', { name: /Renovar/ })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Fechar', exact: true })).toBeInViewport({ ratio: 1 });
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
  await page.screenshot({ path: info.outputPath('changed-query-context.png') });
  await dialog.getByRole('button', { name: 'Fechar', exact: true }).click();
  await expect(dialog).toHaveCount(0);
});

test('an unavailable coffer offers verification instead of switching or requiring OAuth', async ({
  page,
}) => {
  await page.goto('/?fixtureCoffer=unavailable');
  await page.getByRole('tab', { name: /Atenção/ }).click();
  const row = page.locator('.account-row').filter({ hasText: 'Trabalho' });
  await expect(row).toContainText('Cofre indisponível');
  await row.getByRole('button', { name: 'Verificar cofre' }).click();
  await expect(
    page.getByRole('dialog').getByRole('heading', { name: 'Cofre indisponível' }),
  ).toBeVisible();
  await expect(page.getByRole('dialog')).toContainText('não foi invalidada');
  await page.getByRole('button', { name: 'Fechar', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
});
