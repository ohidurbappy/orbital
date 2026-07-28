import { afterAll, beforeAll, describe, expect, it } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequestHandler, resolveServePort, serveUrls } from './serve.js';

describe('resolveServePort', () => {
  it('uses the first bare-number arg', () => {
    expect(resolveServePort(['8080'])).toBe(8080);
    expect(resolveServePort(['--foo', '3000'])).toBe(3000);
  });

  it('falls back to the default when absent or invalid', () => {
    expect(resolveServePort([])).toBe(8000);
    expect(resolveServePort(['notaport'])).toBe(8000);
    expect(resolveServePort(['99999'])).toBe(8000); // out of range
  });
});

describe('serveUrls', () => {
  const ifaces = () =>
    ({
      en0: [{ address: '192.168.1.42', family: 'IPv4', internal: false } as any],
    }) as ReturnType<typeof import('node:os').networkInterfaces>;

  it('builds local and network URLs', () => {
    expect(serveUrls(8080, ifaces)).toEqual({
      local: 'http://localhost:8080',
      network: 'http://192.168.1.42:8080',
    });
  });

  it('returns null network when there is no LAN IPv4', () => {
    expect(serveUrls(8080, () => ({}) as any).network).toBeNull();
  });
});

describe('createRequestHandler', () => {
  let root: string;
  const get = (path: string, handler: (r: Request) => Response) =>
    handler(new Request(`http://localhost${path}`));

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'orb-serve-'));
    writeFileSync(join(root, 'hello.txt'), 'hi there');
    mkdirSync(join(root, 'sub'));
    writeFileSync(join(root, 'sub', 'index.html'), '<h1>sub index</h1>');
  });

  afterAll(() => rmSync(root, { recursive: true, force: true }));

  it('serves a file with its contents', async () => {
    const res = get('/hello.txt', createRequestHandler(root));
    expect(res.status).toBe(200);
    expect(await res.text()).toBe('hi there');
  });

  it('lists a directory without an index', async () => {
    const res = get('/', createRequestHandler(root));
    expect(res.status).toBe(200);
    const html = await res.text();
    expect(html).toContain('Directory listing for /');
    expect(html).toContain('hello.txt');
    expect(html).toContain('sub/');
  });

  it('serves index.html for a directory that has one', async () => {
    const res = get('/sub', createRequestHandler(root));
    expect(await res.text()).toContain('sub index');
  });

  it('404s a missing path', () => {
    expect(get('/nope.txt', createRequestHandler(root)).status).toBe(404);
  });

  it('does not leak files outside the root via traversal', async () => {
    // The URL parser normalizes `..` (raw or percent-encoded) before our guard,
    // so the path resolves under root and simply 404s — never serving /etc/passwd.
    const res = get('/../../etc/passwd', createRequestHandler(root));
    expect(res.status).toBe(404);
    expect(res.status).not.toBe(200);
  });
});
