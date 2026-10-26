import { expect, test } from '@playwright/test';

for (const width of [860, 1180, 1600])
  for (const theme of ['light', 'dark'] as const) {
    test(`review frames · ${width} · ${theme}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await page.clock.setFixedTime(new Date('2026-10-07T12:00:00Z'));
      await page.goto('/?fixtureAccounts=14&fixtureLabels=long');
      await expect(page.locator('.account-row')).toHaveCount(13);
      await page.getByRole('button', { name: 'Selecionar contas', exact: true }).click();
      await page.screenshot({ path: info.outputPath('accounts-selection.png') });
      await page
        .locator('.account-row')
        .first()
        .getByRole('button', { name: /Mais ações/ })
        .click();
      await expect(page.getByRole('menu')).toBeVisible();
      await page
        .getByRole('menu')
        .locator('img')
        .evaluateAll((images) =>
          Promise.all(images.map((image) => (image as HTMLImageElement).decode())),
        );
      await page.screenshot({ path: info.outputPath('accounts-menu.png') });
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Cards', exact: true }).click();
      const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
      await card.getByRole('checkbox').check();
      await expect(
        page.getByRole('button', { name: 'Excluir seleção (1)', exact: true }),
      ).toBeEnabled();
      await card.scrollIntoViewIfNeeded();
      await page.screenshot({ path: info.outputPath('accounts-cards.png') });
      await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
      await expect(
        page.getByRole('heading', { name: 'Suas aplicações, suas contas' }),
      ).toBeVisible();
      await page.screenshot({ path: info.outputPath('applications-empty.png') });
      await page.getByRole('button', { name: /Mostrar contas/ }).click();
      await page.screenshot({ path: info.outputPath('applications-accounts.png') });
      await page
        .locator('.resource-row')
        .filter({ hasText: 'Identidade de teste com nome extenso' })
        .getByRole('button', { name: 'Configurar', exact: true })
        .click();
      const dialog = page.getByRole('dialog');
      await expect(dialog.getByRole('radio', { name: /Só Codex/ })).toBeFocused();
      await expect(
        dialog.getByRole('group', { name: 'Onde a conta pode ser usada' }),
      ).toBeVisible();
      await page.screenshot({ path: info.outputPath('applications-policy.png') });
    });
  }
