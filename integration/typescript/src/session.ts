import type { Client } from './index.js';
import { ManagerError } from './index.js';

export interface ServerMetadata {
  id: string;
  accountId: string;
  applicationId: string;
  state: string;
  eventsOffset?: number;
}
export interface HarnessMessage {
  id?: number | string;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code: number; message: string };
}
export type HostHandler = (request: HarnessMessage) => unknown | Promise<unknown>;
const terminal = new Set(['closed', 'failed', 'interrupted']);

/** One reader per pinned session. The host—not the manager—handles its approvals. */
export class ServerSession {
  readonly id: string;
  private offset: number;
  private tail = '';
  private rpcId = 0;
  private reading = false;
  private readonly buffered: HarnessMessage[] = [];
  constructor(
    private readonly client: Client,
    readonly metadata: ServerMetadata,
  ) {
    this.id = metadata.id;
    this.offset = metadata.eventsOffset ?? 0;
  }
  send(message: HarnessMessage): Promise<unknown> {
    return this.client.request('session/send', { sessionId: this.id, message });
  }
  respond(id: number | string, result: unknown): Promise<unknown> {
    return this.send({ id, result });
  }
  async rpc(
    method: string,
    params: unknown = {},
    options: { hostHandler?: HostHandler; timeoutMs?: number } = {},
  ): Promise<unknown> {
    this.acquireReader();
    try {
      const id = ++this.rpcId;
      await this.send({ id, method, params });
      for await (const message of this.messages(Date.now() + (options.timeoutMs ?? 120000))) {
        if (!message.method && message.id === id) {
          if (message.error) throw new ManagerError('rpc_error', message.error.message);
          return message.result;
        }
        if (message.method && message.id !== undefined) {
          if (!options.hostHandler) {
            await this.close();
            throw new ManagerError(
              'host_request_unhandled',
              "Provide your application's tool/approval handler",
            );
          }
          await this.respond(message.id, await options.hostHandler(message));
        } else {
          if (this.buffered.length >= 256) {
            await this.close();
            throw new ManagerError('event_buffer_full', 'Consume notifications before continuing');
          }
          this.buffered.push(message);
        }
      }
      throw new ManagerError('session_closed', 'Session ended before its response');
    } finally {
      this.reading = false;
    }
  }
  async *events(timeoutMs = 120000): AsyncGenerator<HarnessMessage> {
    this.acquireReader();
    try {
      while (this.buffered.length) yield this.buffered.shift()!;
      yield* this.messages(Date.now() + timeoutMs);
    } finally {
      this.reading = false;
    }
  }
  close(): Promise<unknown> {
    return this.client.request('session/close', { sessionId: this.id });
  }
  private acquireReader(): void {
    if (this.reading) throw new ManagerError('reader_busy', 'Use one event reader per session');
    this.reading = true;
  }
  private async *messages(deadline: number): AsyncGenerator<HarnessMessage> {
    while (Date.now() < deadline) {
      const page = await this.client.request<{ chunk: string; nextOffset: number; state: string }>(
        'session/events',
        { sessionId: this.id, offset: this.offset },
      );
      this.offset = page.nextOffset;
      this.tail += page.chunk;
      let newline = this.tail.indexOf('\n');
      while (newline >= 0) {
        const line = this.tail.slice(0, newline);
        this.tail = this.tail.slice(newline + 1);
        yield JSON.parse(line) as HarnessMessage;
        newline = this.tail.indexOf('\n');
      }
      if (Buffer.byteLength(this.tail) > 262144)
        throw new ManagerError('frame_too_large', 'App Server frame exceeds limit');
      if (!page.chunk && terminal.has(page.state)) return;
      await new Promise<void>((resolve) => setTimeout(resolve, 50));
    }
    await this.close();
    throw new ManagerError('timeout', 'No automatic retry: the operation may have started');
  }
}
