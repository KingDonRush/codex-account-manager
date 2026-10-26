import { tick } from 'svelte';
type DialogOptions = { escape: () => void };
function focusable(node: HTMLElement): HTMLElement[] {
  return Array.from(
    node.querySelectorAll<HTMLElement>('button,input,select,textarea,a[href],[tabindex]'),
  ).filter(
    (item) => !item.matches(':disabled,[tabindex="-1"]') && item.getClientRects().length > 0,
  );
}
/** Keep keyboard focus local; the owner decides whether Escape may cancel/close. */
export function dialogFocus(node: HTMLElement, options: DialogOptions): { destroy: () => void } {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let disposed = false;
  void tick().then(() => {
    if (!disposed) (focusable(node)[0] ?? node).focus();
  });
  const keys = (event: KeyboardEvent): void => {
    if (!ownsFocus()) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      options.escape();
      return;
    }
    if (event.key !== 'Tab' && event.code !== 'Tab') return;
    const items = focusable(node);
    const first = items[0];
    const last = items.at(-1);
    if (!first) {
      event.preventDefault();
      node.focus();
      return;
    }
    const outside = !node.contains(document.activeElement);
    if (
      event.shiftKey &&
      (outside || document.activeElement === first || document.activeElement === node)
    ) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && (outside || document.activeElement === last)) {
      event.preventDefault();
      first.focus();
    }
  };
  function ownsFocus(): boolean {
    return (
      !disposed && Array.from(document.querySelectorAll('[aria-modal="true"]')).at(-1) === node
    );
  }
  const containFocus = (event: FocusEvent): void => {
    if (ownsFocus() && event.target instanceof Node && !node.contains(event.target))
      (focusable(node)[0] ?? node).focus();
  };
  // GTK may send a boundary Tab after moving focus to the page. Capture only
  // the active dialog's navigation, with teardown; never hook OS keyboard input.
  document.addEventListener('keydown', keys, true);
  document.addEventListener('focusin', containFocus, true);
  return {
    destroy: () => {
      disposed = true;
      document.removeEventListener('keydown', keys, true);
      document.removeEventListener('focusin', containFocus, true);
      void tick().then(() => {
        if (previous?.isConnected && !document.querySelector('[aria-modal="true"]'))
          previous.focus();
      });
    },
  };
}
