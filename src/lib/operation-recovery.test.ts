import { it, expect } from 'vitest';
import { recoveryChoices } from './operation-recovery';
it('browser cleanup has a scoped retry without repeating OAuth or switching accounts', () => {
  expect(recoveryChoices('browser_cleanup_pending', true).map((choice) => choice.action)).toEqual([
    'close_browser',
  ]);
});
it('an unconfirmed session after automatic recovery does not send the user back to a read-only check loop', () => {
  expect(recoveryChoices('session_renewal_required', true).map((choice) => choice.action)).toEqual([
    'retry',
    'reconnect',
  ]);
  expect(
    recoveryChoices('session_renewal_required', true)
      .map((choice) => choice.label)
      .join(' '),
  ).not.toMatch(/Verificar e renovar/);
});
it('a protected rotation awaiting the coffer offers recovery diagnostics instead of a new login or replay', () => {
  expect(
    recoveryChoices('credential_recovery_pending', true).map((choice) => choice.action),
  ).toEqual(['diagnostics']);
});
it('Desktop failures recover the launch without repeating a committed switch or requesting OAuth', () => {
  expect(recoveryChoices('desktop_reopen_pending', true).map((choice) => choice.action)).toEqual([
    'open_desktop',
  ]);
  expect(recoveryChoices('desktop_discovery_failed', true).map((choice) => choice.action)).toEqual([
    'open_desktop',
    'diagnostics',
  ]);
  expect(
    recoveryChoices('desktop_close_pending', true).map((choice) => choice.action),
  ).not.toContain('reconnect');
});
it('Windows interruption requires an explicit native confirmation and is separate from retry', () => {
  expect(
    recoveryChoices('desktop_close_pending', true, true).map((choice) => choice.action),
  ).toEqual(['retry', 'confirm_close', 'diagnostics']);
  expect(
    recoveryChoices('desktop_close_pending', true, false).map((choice) => choice.action),
  ).not.toContain('confirm_close');
  expect(
    recoveryChoices('desktop_discovery_failed', true, true).map((choice) => choice.action),
  ).toContain('select_desktop');
});
it('coffer and runtime failures do not propose OAuth or automatic interruption', () => {
  expect(recoveryChoices('credential_unavailable', true).map((choice) => choice.action)).toEqual([
    'check_connection',
  ]);
  expect(recoveryChoices('thread_owned_elsewhere', true).map((choice) => choice.action)).toEqual([
    'diagnostics',
  ]);
});
it('wrong identity distinguishes retrying the original from deliberate replacement', () => {
  expect(recoveryChoices('account_mismatch', true).map((choice) => choice.action)).toEqual([
    'reconnect',
    'replace_account',
  ]);
  expect(recoveryChoices('account_mismatch', false).map((choice) => choice.action)).toEqual([
    'reconnect',
  ]);
});
