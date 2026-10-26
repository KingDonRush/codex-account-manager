export type RefreshScope = 'dashboard' | 'full';

type RefreshLoader = () => Promise<void>;

/**
 * Serializes UI refreshes and folds weaker requests into a stronger one.
 *
 * Tauri events can arrive in a burst: an operation finishes, the manager
 * emits `dashboard-changed`, and the initial command response causes another
 * local refresh. Running those requests concurrently makes the WebView render
 * stale snapshots. An invalidation during a full read must trigger a following
 * dashboard read: the full loader may already have captured the older state.
 */
export class RefreshCoordinator {
  private pending: RefreshScope | null = null;
  private inFlight: Promise<void> | null = null;

  public constructor(
    private readonly loadDashboard: RefreshLoader,
    private readonly loadFull: RefreshLoader,
  ) {}

  public request(scope: RefreshScope): Promise<void> {
    if (scope === 'full') {
      this.pending = 'full';
    } else if (this.pending === null) {
      this.pending = 'dashboard';
    }

    if (this.inFlight === null) {
      this.inFlight = this.drain();
    }
    return this.inFlight;
  }

  private async drain(): Promise<void> {
    try {
      while (this.pending !== null) {
        const scope = this.pending;
        this.pending = null;
        await (scope === 'full' ? this.loadFull() : this.loadDashboard());
      }
    } finally {
      this.inFlight = null;
    }
  }
}
