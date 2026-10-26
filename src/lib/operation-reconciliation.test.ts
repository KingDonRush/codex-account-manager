import { describe, expect, it } from 'vitest';
import { reconcileOperation } from './operation-reconciliation';
import type { LoginOperation } from './types';

function operation(state: LoginOperation['state']): LoginOperation {
  return {
    id: 'operation', profileId: 'profile', kind: 'login', reconnect: false,
    state, step: state, startedAt: new Date(0).toISOString(), stageStartedAt: new Date(0).toISOString(),
    lastStageDurationMs: null, finishedAt: null, errorCode: null, errorMessage: null,
    browserPid: null, runtimeKind: null, durationMs: null, cancelable: state === 'idle',
  };
}

describe('operation snapshots and live events', () => {
  it('shows a fast failure even if the event arrived before invoke returned', () => {
    expect(reconcileOperation(operation('idle'), [operation('failed')])?.state).toBe('failed');
  });
  it('does not regress a newer event to an older in-flight snapshot', () => {
    expect(reconcileOperation(operation('verifying'), [operation('preparing')])?.state).toBe('verifying');
  });
});
