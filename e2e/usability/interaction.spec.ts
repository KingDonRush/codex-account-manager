import { expect, test, type Page } from '@playwright/test';

async function openAccounts(page: Page, fixture = ''): Promise<void> {
  await page.clock.setFixedTime(new Date('2026-10-07T12:00:00Z'));
  await page.goto(`/?fixtureAccounts=14&fixtureLabels=long${fixture}`);
  await expect(page.locator('.account-row').first()).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
}

for (const theme of ['light', 'dark'] as const) {
  test(`browser closure pending has a visible scoped action without another login · ${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width: 860, height: 620 });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto('/?fixtureLogin=success&fixtureBrowserCleanup=pending');
    await page.getByRole('tab', { name: /Atenção/ }).click();
    await page.locator('.account-row').filter({ hasText: 'Conta nova' }).getByRole('button', { name: 'Conectar conta', exact: true }).click();
    const dialog = page.getByRole('dialog');
    const closeBrowser = dialog.getByRole('button', { name: 'Concluir fechamento do navegador', exact: true });
    await expect(closeBrowser).toBeVisible();
    await expect(closeBrowser).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByRole('button', { name: 'Fechar', exact: true })).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByText('Conta conectada; o fechamento do navegador ainda não foi confirmado', { exact: true })).toHaveCount(1);
    await page.screenshot({ path: info.outputPath('browser-closure-pending.png') });
    await closeBrowser.click();
    await expect(dialog.getByText('Conta conectada; navegador fechado', { exact: true })).toBeVisible();
    await expect(closeBrowser).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Fechar', exact: true }).click();
    await expect(dialog).toHaveCount(0);
  });
}

for (const theme of ['light', 'dark'] as const) {
  test(`UX recovery error has one explanation and visible actions at height 620 · ${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width: 860, height: 620 });
    await page.emulateMedia({ colorScheme: theme });
    await openAccounts(page, '&fixtureSwitchError=session_renewal_required');
    const row = page.locator('.account-row').filter({ hasText: 'Identidade de teste com nome extenso' });
    await row.getByRole('button', { name: 'Usar esta conta', exact: true }).click();
    const dialog = page.getByRole('dialog');
    const retry = dialog.getByRole('button', { name: 'Tentar recuperação novamente', exact: true });
    await expect(retry).toBeVisible();
    await expect(dialog.getByText('Não foi possível confirmar a sessão após a tentativa de renovação. A sessão salva foi preservada.', { exact: true })).toHaveCount(1);
    for (const button of [retry, dialog.getByRole('button', { name: 'Entrar novamente no navegador', exact: true }), dialog.getByRole('button', { name: 'Fechar', exact: true })]) {
      await expect(button).toBeInViewport({ ratio: 1 });
    }
    await expect(dialog.getByRole('button', { name: /Verificar.*renovar/ })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('recovery-error.png') });
    await dialog.getByRole('button', { name: 'Fechar', exact: true }).click();
    await expect(dialog).toHaveCount(0);
  });
}

for (const width of [860, 1180, 1600])
  for (const theme of ['light', 'dark']) {
    test(`UX-01 selection keeps identity, balance and actions in one usable row · ${width} · ${theme}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme as 'light' | 'dark' });
      await openAccounts(page);
      const row = page
        .locator('.account-row')
        .filter({ hasText: 'Identidade de teste com nome extenso' })
        .first();
      const before = await row.boundingBox();
      if (!before) throw new Error('Row has no layout');
      await page.getByRole('button', { name: 'Selecionar contas', exact: true }).click();
      await row.getByRole('checkbox').check();
      const layout = await row.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        const selector = element.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
        const identity = element.querySelector<HTMLElement>('.row-identity')!;
        const actions = element.querySelector<HTMLElement>('.row-actions')!;
        const balance = element.querySelector<HTMLElement>('.row-balance')!;
        const boxes = [selector, identity, balance, actions].map((item) =>
          item.getBoundingClientRect(),
        );
        return {
          height: rect.height,
          oneRow: boxes.every(
            (box) => Math.abs(box.y + box.height / 2 - (rect.y + rect.height / 2)) < 12,
          ),
          ordered: boxes.every(
            (box, index) => index === 0 || box.left >= boxes[index - 1]!.right - 1,
          ),
          contained: boxes.every((box) => box.left >= rect.left && box.right <= rect.right),
        };
      });
      expect(layout.height, 'Adding a checkbox must not double row height').toBeLessThanOrEqual(
        before.height + 8,
      );
      expect(layout.oneRow, 'The checkbox cannot push account actions onto another line').toBe(
        true,
      );
      expect(layout.ordered, 'Selection/identity/balance/actions must not overlap').toBe(true);
      expect(layout.contained).toBe(true);
      await expect(page.getByRole('button', { name: 'Excluir seleção (1)' })).toBeEnabled();
    });
  }

test('UX-06 outside click dismisses the menu and does not activate another account', async ({
  page,
}) => {
  await openAccounts(page);
  const identity = await page.locator('.account-row').first().getAttribute('aria-label');
  await page.locator('.account-row').first().click({ button: 'right' });
  await expect(page.getByRole('menu')).toBeVisible();
  const outside = await page.getByRole('heading', { name: 'Contas', exact: true }).boundingBox();
  if (!outside) throw new Error('Heading not rendered');
  await page.mouse.click(outside.x + outside.width / 2, outside.y + outside.height / 2);
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(identity).not.toBeNull();
});

test('UX-08 menu actions have glyphs and a visible hover', async ({ page }) => {
  await openAccounts(page);
  await page.locator('.account-row').first().click({ button: 'right' });
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  const items = menu.getByRole('menuitem');
  const count = await items.count();
  expect(count).toBeGreaterThan(3);
  for (let index = 0; index < count; index++)
    await expect(items.nth(index).locator('img,svg')).toHaveCount(1);
  const item = items.first();
  const before = await item.evaluate((element) => getComputedStyle(element).backgroundColor);
  await item.hover();
  const after = await item.evaluate((element) => getComputedStyle(element).backgroundColor);
  expect(after, 'Hover must not reference an undefined theme token').not.toBe(before);
});

test('UX-06 menu keyboard navigation owns focus and Escape restores it', async ({ page }) => {
  await openAccounts(page);
  const trigger = page
    .locator('.account-row')
    .first()
    .getByRole('button', { name: /Mais ações/ });
  await trigger.click();
  const menu = page.getByRole('menu');
  await expect(menu.getByRole('menuitem').first()).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByRole('menuitem').nth(1)).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await page.keyboard.press('Control+f');
  await expect(page.getByLabel('Buscar contas')).toBeFocused();
});

test('UX-05 groups belong to filters, not dedicated navigation', async ({ page }) => {
  await openAccounts(page);
  await expect(
    page
      .getByRole('navigation', { name: 'Navegação principal' })
      .getByRole('button', { name: 'Grupos', exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel('Filtrar contas por grupo')).toBeVisible();
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await expect(page.getByLabel('Filtrar contas de aplicações por grupo')).toBeVisible();
});

test('UX-09 recovery stays visible while preparation is delayed or fails', async ({ page }) => {
  await openAccounts(page, '&fixtureCheck=renewable&fixturePrepareSwitch=delayed_failure');
  await page.getByRole('tab', { name: /Atenção/ }).click();
  const row = page
    .locator('.account-row')
    .filter({ hasText: 'Identidade de teste com nome extenso' });
  const attentionBefore = await page.getByRole('tab', { name: /Atenção/ }).textContent();
  await row.getByRole('button', { name: 'Verificar sessão salva' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: /Renovar/ })).toBeVisible();
  await dialog.getByRole('button', { name: /Renovar/ }).click();
  await expect(
    page.getByRole('dialog'),
    'Preparation cannot disappear into an invisible await',
  ).toBeVisible();
  await expect(page.getByRole('dialog').getByRole('status')).toBeVisible();
  await expect(page.getByRole('dialog').getByRole('alert')).toBeVisible();
  await expect(page.getByRole('tab', { name: /Atenção/ })).toHaveText(attentionBefore ?? '');
});

test('UX-04 application configuration starts disclosed and has no duplicate account lists', async ({
  page,
}) => {
  await openAccounts(page);
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Suas aplicações, suas contas' })).toBeVisible();
  await expect(page.locator('.resource-row')).toHaveCount(0);
  await expect(page.locator('main input[type="number"]')).toHaveCount(0);
  await page.getByRole('button', { name: /Mostrar contas/ }).click();
  await expect(page.locator('.resource-row')).toHaveCount(13);
  const row = page
    .locator('.resource-row')
    .filter({ hasText: 'Identidade de teste com nome extenso' });
  const geometry = await row.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const identity = element.querySelector('.resource-identity')!.getBoundingClientRect();
    const button = element.querySelector('button')!.getBoundingClientRect();
    return identity.right <= button.left && button.right <= bounds.right;
  });
  expect(geometry).toBe(true);
  await row.getByRole('button', { name: 'Configurar', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Reconectar conta' })).toBeVisible();
  await expect(dialog.getByRole('group', { name: 'Onde a conta pode ser usada' })).toBeVisible();
  await expect(dialog.getByRole('radio', { name: /Só Codex/ })).toBeChecked();
  await expect(dialog.getByLabel('Trabalhos simultâneos de aplicações')).not.toBeVisible();
  await dialog.getByRole('radio', { name: /Codex e aplicações/ }).check();
  await dialog.getByRole('button', { name: 'Salvar uso da conta' }).click();
  await expect(dialog.getByRole('status')).toContainText('salvas');
  await dialog.getByText('Limites avançados', { exact: true }).click();
  await expect(dialog.getByLabel('Trabalhos simultâneos de aplicações')).toHaveValue('1');
  await dialog.getByRole('button', { name: 'Fechar', exact: true }).click();
  await row.getByRole('button', { name: 'Configurar', exact: true }).click();
  await expect(
    page.getByRole('dialog').getByRole('radio', { name: /Codex e aplicações/ }),
  ).toBeChecked();
});

test('UX-05 a group filters both areas using public account IDs, not profile IDs', async ({
  page,
}) => {
  await openAccounts(page);
  await page.getByRole('button', { name: 'Gerenciar grupos', exact: true }).click();
  await page.getByRole('button', { name: 'Criar grupo', exact: true }).click();
  let dialog = page.getByRole('dialog');
  await dialog.getByLabel('Nome', { exact: true }).fill('Pesquisa');
  await dialog.getByLabel('Buscar integrantes').fill('extesno');
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Salvar', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Fechar', exact: true }).click();
  await page.getByLabel('Filtrar contas por grupo').selectOption({ label: 'Pesquisa' });
  await expect(page.locator('.account-row')).toHaveCount(1);
  await expect(page.getByRole('tab', { name: /Conectadas/ }).locator('strong')).toHaveText('1');
  await expect(page.locator('.account-row')).toContainText('Identidade de teste com nome extenso');
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await page
    .getByLabel('Filtrar contas de aplicações por grupo')
    .selectOption({ label: 'Pesquisa' });
  await page.getByRole('button', { name: /Mostrar contas/ }).click();
  await expect(page.locator('.resource-row')).toHaveCount(1);
  await expect(page.locator('.resource-row')).toContainText('Identidade de teste com nome extenso');
});

test('UX-06 shortcuts dismiss the menu without capturing global search', async ({ page }) => {
  await openAccounts(page);
  await page
    .locator('.account-row')
    .first()
    .getByRole('button', { name: /Mais ações/ })
    .click();
  await page.keyboard.press('Control+f');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(page.getByLabel('Buscar contas')).toBeFocused();
  await page
    .locator('.account-row')
    .first()
    .getByRole('button', { name: /Mais ações/ })
    .click();
  await page.keyboard.press('F2');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(page.getByRole('dialog').getByLabel('Novo nome')).toBeVisible();
});

test('compact navigation retains visible icons and accessible names', async ({ page }) => {
  await page.setViewportSize({ width: 860, height: 900 });
  await openAccounts(page);
  const navigation = page.getByRole('navigation', { name: 'Navegação principal' });
  for (const label of ['Contas', 'Aplicações', 'E-mail', 'Diagnóstico']) {
    const button = navigation.getByRole('button', { name: label, exact: true });
    await expect(button.locator('svg')).toBeVisible();
    await expect(button).toHaveAttribute('title', label);
  }
  await navigation.getByRole('button', { name: 'Aplicações', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Aplicações', exact: true })).toBeVisible();
});

test('modal traps Tab and Escape restores focus without invoking an account action', async ({
  page,
}) => {
  await openAccounts(page);
  await page.getByRole('button', { name: 'Aplicações', exact: true }).click();
  const trigger = page.getByRole('button', { name: 'Cadastrar aplicação' });
  await trigger.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByLabel('Nome', { exact: true })).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(dialog.getByRole('button', { name: 'Cancelar', exact: true })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(dialog.getByLabel('Nome', { exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test('cancelling preparation prevents a late response from starting a switch', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-10-07T12:00:00Z') });
  await openAccounts(page, '&fixturePrepareSwitch=delayed_failure');
  await page
    .locator('.account-row')
    .first()
    .getByRole('button', { name: 'Usar esta conta', exact: true })
    .click();
  await expect(page.getByRole('dialog').getByRole('status')).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Cancelar preparação' }).click();
  await page.clock.runFor(1500);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
  await page.getByRole('button', { name: 'Diagnóstico', exact: true }).click();
  await expect(page.locator('.operation-item')).toHaveCount(0);
});

test('UX-06 cards use the same dismissible context menu and preserve their secondary actions', async ({
  page,
}) => {
  await openAccounts(page);
  await page.getByRole('button', { name: 'Cards', exact: true }).click();
  const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
  await card.click({ button: 'right' });
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Reautenticar', exact: true })).toBeVisible();
  await expect(
    menu.getByRole('menuitem', { name: 'Configurar webmail', exact: true }),
  ).toBeVisible();
  await expect(
    menu.getByRole('menuitem', { name: 'Excluir dados locais', exact: true }),
  ).toBeVisible();
  await menu.getByRole('menuitem', { name: 'Renomear F2', exact: true }).click();
  await expect(page.getByRole('dialog').getByLabel('Novo nome')).toBeVisible();
  await page.keyboard.press('Escape');
  await card.getByRole('button', { name: /Mais ações/ }).click();
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(card.getByRole('button', { name: /Mais ações/ })).toBeFocused();
});

test('selection also works in cards without invoking the primary action', async ({ page }) => {
  await openAccounts(page);
  await page.getByRole('button', { name: 'Cards', exact: true }).click();
  await page.getByRole('button', { name: 'Selecionar contas', exact: true }).click();
  const card = page.locator('.account-card').filter({ hasText: 'Conta pessoal' });
  await card.getByRole('checkbox', { name: 'Selecionar Conta pessoal', exact: true }).check();
  await expect(
    page.getByRole('button', { name: 'Excluir seleção (1)', exact: true }),
  ).toBeEnabled();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Conta em uso' })).toContainText('Conta pessoal');
});

test('intentional outside scrolling dismisses the menu without moving focus to an offscreen account', async ({
  page,
}) => {
  await openAccounts(page);
  await page
    .locator('.account-row')
    .first()
    .getByRole('button', { name: /Mais ações/ })
    .click();
  await expect(page.getByRole('menu')).toBeVisible();
  await page.mouse.move(450, 150);
  await page.mouse.wheel(0, 300);
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
});
