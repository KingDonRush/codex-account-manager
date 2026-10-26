import { expect, test } from '@playwright/test';

for (const theme of ['light', 'dark'] as const) {
  test.describe(`Windows Desktop recovery · ${theme}`, () => {
    test.use({ colorScheme: theme });

    test('a failed post-commit Desktop launch is recovered without another account switch', async ({
      page,
    }) => {
      await page.goto('/?fixtureDesktop=reopen_pending');
      await page.getByRole('button', { name: 'Cards', exact: true }).click();
      await page
        .locator('.account-card')
        .filter({ hasText: 'Trabalho' })
        .getByRole('button', { name: 'Usar esta conta' })
        .click();
      const dialog = page.getByRole('dialog');
      const open = dialog.getByRole('button', { name: 'Abrir Codex novamente' });
      await expect(open).toBeVisible();
      await expect(dialog.locator('.operation-hero')).toHaveClass(/pending/);
      await expect(
        dialog.getByText('A troca continua em segundo plano e esta janela mostra a etapa real.', {
          exact: true,
        }),
      ).toHaveCount(0);
      await dialog.screenshot({ path: `test-results/windows-recovery-pending-${theme}.png` });
      await expect(dialog.getByText('A conta está ativa no Codex.', { exact: true })).toHaveCount(
        0,
      );
      await open.click();
      await expect(open).toBeDisabled();
      await expect(dialog.getByText('Abertura do Codex confirmada', { exact: true })).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-mock-switch-requests', '1');
      await expect(page.locator('html')).toHaveAttribute('data-mock-desktop-requests', '1');
    });

    test('another launch failure stays actionable inside the same modal and never requests OAuth', async ({
      page,
    }) => {
      await page.goto('/?fixtureDesktop=reopen_pending&fixtureDesktopOpen=failure');
      await page.getByRole('button', { name: 'Cards', exact: true }).click();
      await page
        .locator('.account-card')
        .filter({ hasText: 'Trabalho' })
        .getByRole('button', { name: 'Usar esta conta' })
        .click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('button', { name: 'Abrir Codex novamente' }).click();
      await expect(dialog.getByText('A abertura sintética não foi confirmada.')).toBeVisible();
      await expect(
        dialog.getByText('A troca continua em segundo plano e esta janela mostra a etapa real.', {
          exact: true,
        }),
      ).toHaveCount(0);
      await dialog.screenshot({ path: `test-results/windows-recovery-retry-failed-${theme}.png` });
      await expect(dialog.getByRole('button', { name: 'Abrir Codex novamente' })).toBeEnabled();
      await expect(dialog.getByRole('button', { name: /Reconectar no navegador/ })).toHaveCount(0);
      await expect(page.locator('html')).toHaveAttribute('data-mock-switch-requests', '1');
    });

    test('discovery failure preserves the active account and offers detection instead of reauthentication', async ({
      page,
    }) => {
      await page.goto('/?fixtureDesktop=discovery_failed');
      await page.getByRole('button', { name: 'Cards', exact: true }).click();
      const work = page.locator('.account-card').filter({ hasText: 'Trabalho' });
      await work.getByRole('button', { name: 'Usar esta conta' }).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog.getByRole('button', { name: 'Detectar e abrir Codex' })).toBeVisible();
      await expect(dialog.getByRole('button', { name: /Reconectar no navegador/ })).toHaveCount(0);
      await expect(work.getByText('Em uso', { exact: true })).toHaveCount(0);
      await expect(
        page
          .locator('.account-card')
          .filter({ hasText: 'Conta pessoal' })
          .getByText('Em uso', { exact: true }),
      ).toBeVisible();
    });
  });
}
