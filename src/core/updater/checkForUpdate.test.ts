import { describe, expect, it } from 'bun:test';
import { checkForUpdate } from './checkForUpdate.js';

function fakeFetch(body: unknown, ok = true, status = 200): typeof fetch {
  return (async () =>
    ({
      ok,
      status,
      json: async () => body,
    }) as Response) as unknown as typeof fetch;
}

describe('checkForUpdate', () => {
  it('reports an update when the latest tag is greater', async () => {
    const res = await checkForUpdate(
      fakeFetch({ tag_name: 'v1.2.0', html_url: 'http://x', assets: [] }),
      '1.0.0',
    );
    expect(res.hasUpdate).toBe(true);
    expect(res.latest).toBe('1.2.0');
    expect(res.url).toBe('http://x');
  });

  it('reports no update when already current', async () => {
    const res = await checkForUpdate(fakeFetch({ tag_name: 'v1.0.0' }), '1.0.0');
    expect(res.hasUpdate).toBe(false);
  });

  it('reports no update when the response is not ok', async () => {
    const res = await checkForUpdate(fakeFetch({}, false, 404), '1.0.0');
    expect(res.hasUpdate).toBe(false);
    expect(res.latest).toBeNull();
  });

  it('swallows network errors and reports no update', async () => {
    const throwingFetch = (async () => {
      throw new Error('offline');
    }) as unknown as typeof fetch;
    const res = await checkForUpdate(throwingFetch, '1.0.0');
    expect(res.hasUpdate).toBe(false);
    expect(res.current).toBe('1.0.0');
  });

  it('ignores invalid tags', async () => {
    const res = await checkForUpdate(fakeFetch({ tag_name: 'nightly' }), '1.0.0');
    expect(res.hasUpdate).toBe(false);
  });
});
