import { expect, test } from '@playwright/test';

test('unconfirmed switch recovery stays in Use, with one message and no read-only verification loop', async ({ page }, info) => {
  const fixedTime = new Date('2026-10-10T00:00:00Z');
  await page.clock.install({ time: fixedTime });
  await page.clock.pauseAt(fixedTime);
  await page.goto('/?fixtureSwitchError=session_renewal_required');
  const row = page.locator('.account-row').filter({ hasText: 'Trabalho' });
  await row.getByRole('button', { name: 'Usar esta conta', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-mock-switch-requests', '1');
  await page.clock.runFor(350);
  const dialog = page.getByRole('dialog');
  const message = 'Não foi possível confirmar a sessão após a tentativa de renovação. A sessão salva foi preservada.';
  await expect(dialog.getByText(message, { exact: true })).toHaveCount(1);
  await expect(dialog.getByRole('button', { name: /Verificar.*renovar/ })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Entrar novamente no navegador', exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-mock-switch-requests', '1');
  await page.screenshot({ path: info.outputPath('session-recovery-error.png') });
  await dialog.getByRole('button', { name: 'Tentar recuperação novamente', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-mock-switch-requests', '2');
  await page.clock.runFor(110);
  await expect(dialog.getByText('Validando a credencial e reiniciando o Codex', { exact: true })).toBeVisible();
  await page.clock.runFor(250);
  await expect(dialog.getByText('Conta ativada no Codex', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: /Verificar.*renovar/ })).toHaveCount(0);
});

test('old account recovery stays in maintenance and does not demand login', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Diagnóstico', exact: true }).click();
  const recovery = page.getByRole('button', { name: 'Recuperar contas salvas', exact: true });
  await expect(recovery).not.toBeVisible();
  await page.getByText('Manutenção e exportação', { exact: true }).click();
  await recovery.click();
  await expect(
    page.getByText(
      'Nenhuma conta adicional encontrada. As contas já cadastradas foram preservadas.',
      { exact: true },
    ),
  ).toBeVisible();
  await expect(page.getByRole('dialog')).not.toBeVisible();
});

test('recovery preserves the original chat and discloses the affected scope before interruption', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Diagnóstico', exact: true }).click();
  await expect(
    page.getByLabel('Chat que aparece como aberto em outro aplicativo'),
  ).not.toBeVisible();
  await page.getByText('Runtime e chats bloqueados', { exact: true }).click();
  await expect(page.getByText('Servidor sem trabalho ativo', { exact: true })).toBeVisible();
  await page
    .getByLabel('Chat que aparece como aberto em outro aplicativo')
    .fill('codex://threads/01a0dabe-0cc8-7aa0-a327-dd24d42869a3');
  await page.getByRole('button', { name: 'Recuperar chat original', exact: true }).click();
  await expect(page.getByText('2 chats', { exact: true })).toBeVisible();
  const interrupt = page.getByRole('button', { name: 'Interromper servidor e recuperar' });
  await expect(interrupt).toBeDisabled();
  await page.getByLabel('Entendo que o trabalho em andamento pode ser interrompido.').check();
  await interrupt.click();
  await expect(page.getByText('Chat original liberado, sem fork.', { exact: true })).toBeVisible();
});
