import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { ServerSession, type ServerMetadata } from './session.js';

export class ManagerError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ManagerError';
  }
}
export interface GrantScope {
  accountIds: string[];
  directAccountIds?: string[] | null;
  groupIds: string[];
  permissions: string[];
  globalIncludingFuture: boolean;
  backgroundGroupIds: string[];
  expiresAt: string | null;
  permanent: boolean;
}
export interface AccountSelection {
  accountId?: string;
  groupId?: string;
}
export interface ExecuteOptions extends AccountSelection {
  arguments?: string[];
}
export interface Execution {
  id: string;
  applicationId: string;
  bindingId: string;
  accountId: string | null;
  state: string;
  /** Inspect this independently of state: work may have ended with preservation pending. */
  errorCode?: string | null;
  exitCode?: number | null;
}
interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class Client {
  private readonly process: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<number, Pending>();
  private id = 0;
  private tail = '';
  private closed = false;
  private connectedApplicationId: string | null = null;
  get applicationId(): string | null {
    return this.connectedApplicationId;
  }
  private rememberApplication(receipt: unknown): unknown {
    const value = receipt as { authenticated?: unknown; applicationId?: unknown } | null;
    if (value?.authenticated === true && typeof value.applicationId === 'string')
      this.connectedApplicationId = value.applicationId;
    return receipt;
  }
  constructor(executable = 'codex-account-manager-cli', arguments_: string[] = []) {
    this.process = spawn(executable, arguments_, {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.process.stdin.on('error', (error) => this.fail(error));
    this.process.stdout.setEncoding('utf8');
    this.process.stdout.on('data', (chunk: string) => this.read(chunk));
    this.process.stderr.on('data', (chunk: Buffer) => process.stderr.write(chunk));
    this.process.on('error', (error) => this.fail(error));
    this.process.on('exit', () =>
      this.fail(new ManagerError('disconnected', 'Local manager connection closed')),
    );
  }
  request<T = unknown>(method: string, params: unknown = {}, timeoutMs = 180000): Promise<T> {
    if (this.closed) return Promise.reject(new ManagerError('closed', 'Client closed'));
    if (this.pending.size >= 32)
      return Promise.reject(new ManagerError('client_busy', 'Too many requests in flight'));
    const id = ++this.id;
    const line = JSON.stringify({ id, method, params });
    if (Buffer.byteLength(line) > 262144)
      return Promise.reject(new ManagerError('frame_too_large', 'Request exceeds limit'));
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new ManagerError(
            'timeout',
            'Request timed out; do not automatically repeat an execution',
          ),
        );
        this.close();
      }, timeoutMs);
      this.pending.set(id, { resolve: (value) => resolve(value as T), reject, timer });
      this.process.stdin.write(`${line}\n`, (error) => {
        if (error) this.fail(error);
      });
    });
  }
  connect(
    name: string,
    slug: string,
    scope: Partial<GrantScope> = {},
    applicationId: string | null = null,
  ): Promise<unknown> {
    return this.request('client/connect', {
      name,
      slug,
      applicationId: applicationId ?? this.connectedApplicationId,
      requestedScope: {
        accountIds: [],
        groupIds: [],
        permissions: ['read_metadata', 'execute'],
        globalIncludingFuture: false,
        backgroundGroupIds: [],
        expiresAt: null,
        permanent: false,
        ...scope,
      },
    }).then((receipt) => this.rememberApplication(receipt));
  }
  /** Execute directly with the resources approved in the connection dialog. */
  execute(prompt: string, cwd: string, options: ExecuteOptions = {}): Promise<Execution> {
    return this.request('execution/start', {
      prompt,
      cwd,
      arguments: options.arguments ?? [],
      accountId: options.accountId ?? null,
      groupId: options.groupId ?? null,
    });
  }
  /** Compatibility for clients with a saved bindingId; new clients use execute. */
  exec(
    bindingId: string,
    prompt: string,
    cwd: string,
    arguments_: string[] = [],
    accountId: string | null = null,
  ): Promise<Execution> {
    return this.request('execution/start', {
      bindingId,
      prompt,
      cwd,
      arguments: arguments_,
      accountId,
    });
  }
  cancel(executionId: string): Promise<unknown> {
    return this.request('execution/cancel', { executionId });
  }
  authorize(
    name: string,
    slug: string,
    scope: Partial<GrantScope> = {},
    applicationId: string | null = null,
  ): Promise<unknown> {
    return this.request('client/authorize', {
      name,
      slug,
      applicationId: applicationId ?? this.connectedApplicationId,
      requestedScope: {
        accountIds: [],
        groupIds: [],
        permissions: ['read_metadata', 'execute'],
        globalIncludingFuture: false,
        backgroundGroupIds: [],
        expiresAt: null,
        permanent: false,
        ...scope,
      },
    }).then((receipt) => this.rememberApplication(receipt));
  }
  /** Open a pinned App Server directly; omitted selection uses approved resources. */
  async openSession(
    cwd: string,
    options: AccountSelection & { persistHistory?: boolean } = {},
  ): Promise<ServerSession> {
    const metadata = await this.request<ServerMetadata>('session/open', {
      cwd,
      accountId: options.accountId ?? null,
      groupId: options.groupId ?? null,
      persistHistory: options.persistHistory ?? false,
    });
    return new ServerSession(this, metadata);
  }
  /** Compatibility for clients with a saved bindingId; new clients use openSession. */
  async session(
    bindingId: string,
    cwd: string,
    options: { accountId?: string; persistHistory?: boolean } = {},
  ): Promise<ServerSession> {
    const metadata = await this.request<ServerMetadata>('session/open', {
      bindingId,
      cwd,
      accountId: options.accountId ?? null,
      persistHistory: options.persistHistory ?? false,
    });
    return new ServerSession(this, metadata);
  }
  async *events(executionId: string): AsyncGenerator<Record<string, unknown>> {
    let offset = 0;
    let tail = '';
    while (!this.closed) {
      const page = await this.request<{ chunk: string; nextOffset: number; state: string }>(
        'execution/events',
        { executionId, offset },
      );
      offset = page.nextOffset;
      tail += page.chunk;
      let boundary = tail.indexOf('\n');
      while (boundary >= 0) {
        const line = tail.slice(0, boundary);
        tail = tail.slice(boundary + 1);
        if (line.trim()) yield JSON.parse(line) as Record<string, unknown>;
        boundary = tail.indexOf('\n');
      }
      if (!page.chunk && ['completed', 'failed', 'cancelled', 'interrupted'].includes(page.state)) {
        await this.request('execution/ack', { executionId });
        return;
      }
      await new Promise<void>((resolve) => setTimeout(resolve, 100));
    }
  }
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.process.stdin.end();
    const timer = setTimeout(() => this.process.kill(), 2000);
    timer.unref();
    this.process.once('exit', () => clearTimeout(timer));
    this.fail(new ManagerError('closed', 'Client closed'));
  }
  private read(chunk: string): void {
    this.tail += chunk;
    if (Buffer.byteLength(this.tail) > 262144) {
      this.fail(new ManagerError('frame_too_large', 'Manager response exceeds limit'));
      this.close();
      return;
    }
    let boundary = this.tail.indexOf('\n');
    while (boundary >= 0) {
      const line = this.tail.slice(0, boundary);
      this.tail = this.tail.slice(boundary + 1);
      this.deliver(line);
      boundary = this.tail.indexOf('\n');
    }
  }
  private deliver(line: string): void {
    try {
      const response = JSON.parse(line) as {
        id: number;
        result: unknown;
        error: { code: string; message: string } | null;
      };
      const pending = this.pending.get(response.id);
      if (!pending) throw new ManagerError('protocol_error', 'Unknown correlation');
      this.pending.delete(response.id);
      clearTimeout(pending.timer);
      if (response.error)
        pending.reject(new ManagerError(response.error.code, response.error.message));
      else pending.resolve(response.result);
    } catch (error) {
      this.fail(error instanceof Error ? error : new Error(String(error)));
    }
  }
  private fail(error: Error): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    if (!this.closed) this.close();
  }
}
export { ServerSession };
export type { ServerMetadata, HarnessMessage, HostHandler } from './session.js';
