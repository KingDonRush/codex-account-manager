import { expect, test } from '@playwright/test';

test('approving a connected account finishes access without a second OAuth action', async ({
  page,
}, info) => {
  await page.goto('/?fixtureConsent=pending&fixtureAccountAuth=shared');
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox', { name: 'Autorizar Conta pessoal', exact: true }).check();
  await dialog.getByRole('button', { name: 'Autorizar e concluir acesso' }).click();
  const resource = page.locator('.authorized-resource');
  await expect(resource).toHaveCount(1);
  await expect(resource).toContainText('Usa a conta conectada');
  await expect(resource.getByRole('button', { name: 'Conectar', exact: true })).toHaveCount(0);
  await resource.getByRole('button', { name: 'Configurar conta' }).click();
  await expect(
    page.getByRole('dialog').getByRole('button', { name: /Conectar|Reconectar/ }),
  ).toHaveCount(0);
  await page.screenshot({ path: info.outputPath('shared-account-no-second-login.png') });
});

for (const theme of ['light', 'dark'] as const) {
  test(`execution preservation stays actionable without replay · ${theme}`, async ({
    page,
  }, info) => {
    await page.setViewportSize({ width: 860, height: 620 });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto('/?fixtureExecutions=pending');
    await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
    const activity = page.getByRole('region', { name: 'Atividade de aplicações' });
    await activity.locator('summary').filter({ hasText: 'Trabalhos' }).click();
    const row = activity.locator('.activity-row').nth(2);
    const recovery = row.getByRole('button', { name: 'Concluir gravação no cofre', exact: true });
    await recovery.scrollIntoViewIfNeeded();
    await expect(recovery).toBeInViewport({ ratio: 1 });
    await expect(row).toContainText('Cancelado · gravação no cofre pendente');
    const contained = await row.evaluate((element) => {
      const outer = element.getBoundingClientRect();
      return [...element.querySelectorAll('strong,small,button')].every((child) => {
        const bounds = child.getBoundingClientRect();
        return bounds.left >= outer.left && bounds.right <= outer.right;
      });
    });
    expect(contained).toBe(true);
    await page.screenshot({ path: info.outputPath('execution-preservation-pending.png') });
    await recovery.click();
    await expect(row).toBeVisible();
    await expect(row).toContainText('Cancelado · houve pendência de sessão');
    await expect(activity.locator('.activity-row')).toHaveCount(3);
    await expect(
      activity.getByRole('button', { name: 'Concluir gravação no cofre', exact: true }),
    ).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('execution-preservation-history.png') });
  });
}

for (const width of [860, 1180, 1600])
  for (const theme of ['light', 'dark'] as const) {
    test(`application consent, resources and visible account usage · ${width} · ${theme}`, async ({
      page,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto(
        '/?fixtureConsent=pending&fixtureAccounts=14&fixtureLabels=long&fixturePlans=mixed',
      );
      const dialog = page.getByRole('dialog');
      await expect(
        dialog.getByRole('heading', { name: 'Conectar Aplicação de demonstração' }),
      ).toBeVisible();
      await expect(
        dialog.getByRole('button', { name: 'Autorizar e concluir acesso' }),
      ).toBeInViewport();
      await expect(dialog.getByRole('button', { name: 'Negar', exact: true })).toBeInViewport();
      await dialog.getByLabel('Buscar contas para autorização').focus();
      await page.keyboard.press('Shift+Tab');
      // The disabled approve action is skipped until a resource is selected.
      await expect(dialog.getByRole('button', { name: 'Negar', exact: true })).toBeFocused();
      await page.keyboard.press('Tab');
      await expect(dialog.getByLabel('Buscar contas para autorização')).toBeFocused();
      await expect(
        dialog.getByRole('checkbox', { name: 'Trocar a conta do Codex' }),
      ).not.toBeVisible();
      await dialog.getByRole('checkbox', { name: 'Autorizar Conta pessoal', exact: true }).check();
      await dialog.getByRole('checkbox', { name: /Autorizar Identidade de teste/ }).check();
      await dialog.getByLabel('Buscar contas para autorização').fill('conta de tsete 1');
      await expect(dialog.locator('.consent-account').first()).toContainText('Conta de teste 1');
      await dialog.getByLabel('Buscar contas para autorização').fill('');
      await expect(dialog.locator('.plan-badge').filter({ hasText: 'Pro' }).first()).toBeVisible();
      const contained = await dialog
        .locator('.consent-account')
        .first()
        .evaluate((element) => {
          const parent = element.getBoundingClientRect();
          return [...element.children].every((child) => {
            const box = child.getBoundingClientRect();
            return box.left >= parent.left && box.right <= parent.right;
          });
        });
      expect(contained).toBe(true);
      await page.screenshot({ path: info.outputPath('consent.png') });
      await dialog.getByRole('button', { name: 'Autorizar e concluir acesso' }).click();
      await expect(page.locator('.authorized-resource')).toHaveCount(2);
      await expect(page.getByRole('button', { name: 'Adicionar vínculo' })).toHaveCount(0);
      await expect(page.locator('.resource-row')).toHaveCount(0);
      const resources = await page.locator('.authorized-resources').evaluate((element) => {
        const box = element.getBoundingClientRect();
        const parent = element.parentElement!.getBoundingClientRect();
        return box.width / parent.width;
      });
      expect(
        resources,
        'Authorized accounts must use the body width, not a squeezed side column',
      ).toBeGreaterThan(0.85);
      await page.screenshot({ path: info.outputPath('application-access.png') });
      const resource = page
        .locator('.authorized-resource')
        .filter({ hasText: 'Identidade de teste' });
      await resource.getByRole('button', { name: 'Configurar conta' }).click();
      await expect(
        page.getByRole('dialog').getByRole('radio', { name: /Codex e aplicações/ }),
      ).toBeChecked();
      await expect(
        page.getByRole('dialog').getByLabel('Trabalhos simultâneos de aplicações'),
      ).not.toBeVisible();
      const choices = await page.locator('.usage-choices label').evaluateAll((labels) =>
        labels.map((element) => {
          const box = element.getBoundingClientRect();
          return [...element.querySelectorAll('input,svg,strong,small')].every((child) => {
            const item = child.getBoundingClientRect();
            return (
              item.left >= box.left &&
              item.right <= box.right &&
              item.top >= box.top &&
              item.bottom <= box.bottom
            );
          });
        }),
      );
      expect(
        choices,
        'Every usage option must keep its radio, icon and readable copy inside the card',
      ).toEqual([true, true, true]);
      await page.screenshot({ path: info.outputPath('account-usage.png') });
    });
  }

test('consent denial creates neither a registered application nor access', async ({ page }) => {
  await page.goto('/?fixtureConsent=pending');
  await page.getByRole('dialog').getByRole('button', { name: 'Negar', exact: true }).click();
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await expect(page.locator('.application-card')).toHaveCount(0);
  await expect(page.locator('.authorized-resource')).toHaveCount(0);
});
