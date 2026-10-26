import { describe, expect, it } from 'vitest';
import { RefreshCoordinator } from './refresh-coordinator';

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolvePromise: (() => void) | undefined;
  const promise = new Promise<void>((resolve) => {
    resolvePromise = resolve;
  });
  return {
    promise,
    resolve: () => resolvePromise?.(),
  };
}

describe('RefreshCoordinator', () => {
  it('coalesces a terminal refresh and a dashboard event into one full load', async () => {
    const dashboard = deferred();
    const calls: string[] = [];
    const coordinator = new RefreshCoordinator(
      async () => {
        calls.push('dashboard');
        await dashboard.promise;
      },
      async () => {
        calls.push('full');
      },
    );

    const first = coordinator.request('dashboard');
    const second = coordinator.request('full');
    dashboard.resolve();
    await Promise.all([first, second]);

    expect(calls).toEqual(['dashboard', 'full']);
  });

  it('reads a dashboard invalidated after the full loader captured its snapshot', async () => {
    const full = deferred();
    const calls: string[] = [];
    const coordinator = new RefreshCoordinator(
      async () => {
        calls.push('dashboard');
      },
      async () => {
        calls.push('full');
        await full.promise;
      },
    );

    const first = coordinator.request('full');
    const second = coordinator.request('dashboard');
    full.resolve();
    await Promise.all([first, second]);

    expect(calls).toEqual(['full', 'dashboard']);
  });

  it('runs a new request after the previous refresh settles', async () => {
    const calls: string[] = [];
    const coordinator = new RefreshCoordinator(
      async () => {
        calls.push('dashboard');
      },
      async () => {
        calls.push('full');
      },
    );

    await coordinator.request('dashboard');
    await coordinator.request('dashboard');

    expect(calls).toEqual(['dashboard', 'dashboard']);
  });

  it('recovers after a loader rejects', async () => {
    let attempts = 0;
    const coordinator = new RefreshCoordinator(
      async () => {
        attempts += 1;
        if (attempts === 1) throw new Error('temporary failure');
      },
      async () => undefined,
    );

    await expect(coordinator.request('dashboard')).rejects.toThrow('temporary failure');
    await expect(coordinator.request('dashboard')).resolves.toBeUndefined();
    expect(attempts).toBe(2);
  });
});
