import type { LoginOperation, OperationState } from './types';

const STATE_ORDER: Record<OperationState, number> = {
  idle: 0, preparing: 1, browser_launching: 2, awaiting_login: 3,
  verifying: 4, awaiting_confirmation: 5, persisting: 6, completed: 7, cancelled: 7, failed: 7, timed_out: 7,
};

export function reconcileOperation(
  current: LoginOperation | null,
  persisted: LoginOperation[],
): LoginOperation | null {
  if (!current) return null;
  const candidate = persisted.find((operation) => operation.id === current.id);
  if (!candidate || STATE_ORDER[candidate.state] < STATE_ORDER[current.state]) return current;
  // A worker can finish before invoke() returns its initial Idle snapshot;
  // events can also advance while an older snapshot is in flight.
  return candidate;
}
