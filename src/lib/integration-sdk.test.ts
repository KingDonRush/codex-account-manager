import { it, expect } from 'vitest';
import { resolve } from 'node:path';
import { Client } from '../../integration/typescript/src/index.js';
const fixture = resolve('integration/python/fake_bridge.py');
it('authorization changes retain the authenticated app ID instead of creating another app', async () => {
  const client = new Client('python3', [fixture]);
  try {
    await client.connect('App', 'app');
    await client.authorize('App', 'app', { permissions: ['manage_groups'] });
    const requests =
      await client.request<
        {
          method: string;
          params: { applicationId: string; requestedScope: { permissions: string[] } };
        }[]
      >('fixture/requests');
    expect(client.applicationId).toBe('application');
    expect(requests[1]?.params.applicationId).toBe('application');
    expect(requests[1]?.params.requestedScope.permissions).toEqual(['manage_groups']);
  } finally {
    client.close();
  }
});
it('direct execution and harness use approved resources without registering a binding', async () => {
  const client = new Client('python3', [fixture]);
  try {
    await client.execute('synthetic', process.cwd(), { accountId: 'approved' });
    const session = await client.openSession(process.cwd(), { groupId: 'approved-group' });
    await session.close();
    const requests =
      await client.request<{ method: string; params: Record<string, unknown> }[]>(
        'fixture/requests',
      );
    const submitted = requests.filter((request) =>
      ['execution/start', 'session/open'].includes(request.method),
    );
    expect(submitted).toHaveLength(2);
    expect(submitted.every((request) => !('bindingId' in request.params))).toBe(true);
    expect(submitted[0]?.params.accountId).toBe('approved');
    expect(submitted[1]?.params.groupId).toBe('approved-group');
  } finally {
    client.close();
  }
});
it('App Server harness retains identity and delegates host decisions explicitly', async () => {
  const client = new Client('python3', [fixture]);
  try {
    await client.connect('fixture', 'fixture');
    expect(
      await client.authorize(
        'fixture',
        'fixture',
        { permissions: ['manage_groups'] },
        'application',
      ),
    ).toMatchObject({ authenticated: true });
    const session = await client.session('binding', process.cwd());
    expect(await session.rpc('thread/start', {})).toEqual({ thread: { id: 'owned' } });
    const requests: string[] = [];
    const result = await session.rpc(
      'turn/start',
      { threadId: 'owned' },
      {
        hostHandler: async (request) => {
          requests.push(request.method!);
          return { decision: 'decline' };
        },
      },
    );
    expect(result).toEqual({ accepted: true, hostResult: { decision: 'decline' } });
    expect(requests).toEqual(['item/tool/requestUserInput']);
    expect(session.metadata.accountId).toBe('pinned-account');
    await session.close();
    const messages = [];
    for await (const message of session.events()) messages.push(message);
    expect(messages.some((message) => message.method === 'turn/completed')).toBe(true);
  } finally {
    client.close();
  }
});
it('missing tool handler never implicitly authorizes a host request', async () => {
  const client = new Client('python3', [fixture]);
  try {
    const session = await client.session('binding', process.cwd());
    await expect(session.rpc('turn/start', {})).rejects.toMatchObject({
      code: 'host_request_unhandled',
    });
  } finally {
    client.close();
  }
});
