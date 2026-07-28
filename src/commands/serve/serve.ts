import os from 'node:os';
import { readdirSync, statSync } from 'node:fs';
import { isAbsolute, join, normalize, relative } from 'node:path';
import { getLocalIps, getLocalIpv4 } from '../ip/ip.js';

/** The running server handle returned by `Bun.serve`. */
export type ServeServer = ReturnType<typeof Bun.serve>;

/** Default port when none is given, matching `python -m http.server`. */
export const DEFAULT_PORT = 8000;

/** Pick the port from the CLI args (first bare number), else the default. */
export function resolveServePort(args: string[] = [], fallback = DEFAULT_PORT): number {
  const token = args.find((a) => /^\d+$/.test(a));
  const n = token ? Number(token) : fallback;
  return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : fallback;
}

export interface ServeUrls {
  /** Loopback URL for this machine. */
  local: string;
  /** LAN URL other devices can reach, or null when offline. */
  network: string | null;
}

/** The URLs the server is reachable at — localhost plus the LAN IPv4. */
export function serveUrls(
  port: number,
  networkInterfaces: typeof os.networkInterfaces = os.networkInterfaces,
): ServeUrls {
  const lan = getLocalIpv4(getLocalIps(networkInterfaces));
  return {
    local: `http://localhost:${port}`,
    network: lan ? `http://${lan.address}:${port}` : null,
  };
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function directoryListing(dirPath: string, urlPath: string): Response {
  const entries = readdirSync(dirPath, { withFileTypes: true }).sort((a, b) => {
    // Directories first, then alphabetical — like a familiar file listing.
    if (a.isDirectory() !== b.isDirectory()) return a.isDirectory() ? -1 : 1;
    return a.name.localeCompare(b.name);
  });

  const base = urlPath.endsWith('/') ? urlPath : `${urlPath}/`;
  const rows = entries.map((e) => {
    const name = e.isDirectory() ? `${e.name}/` : e.name;
    return `<li><a href="${encodeURI(base + name)}">${escapeHtml(name)}</a></li>`;
  });
  if (urlPath !== '/') rows.unshift(`<li><a href="${encodeURI(base + '..')}">../</a></li>`);

  const title = `Directory listing for ${escapeHtml(urlPath)}`;
  const html = `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title></head>
<body><h1>${title}</h1><ul>${rows.join('')}</ul></body></html>`;
  return new Response(html, { headers: { 'Content-Type': 'text/html; charset=utf-8' } });
}

/**
 * Build a request handler that serves files from `root`. Directories return an
 * index.html if present, otherwise an HTML listing; missing paths 404; attempts
 * to escape `root` via `..` are refused. Kept free of `Bun.serve` so it can be
 * unit-tested by passing plain `Request`s.
 */
export function createRequestHandler(root: string): (req: Request) => Response {
  const normalizedRoot = normalize(root);

  return function handle(req: Request): Response {
    const pathname = decodeURIComponent(new URL(req.url).pathname);
    const target = normalize(join(normalizedRoot, pathname));

    // Refuse anything that resolves outside the served root.
    const rel = relative(normalizedRoot, target);
    if (rel.startsWith('..') || isAbsolute(rel)) {
      return new Response('Forbidden', { status: 403 });
    }

    let stat;
    try {
      stat = statSync(target);
    } catch {
      return new Response('Not Found', { status: 404 });
    }

    if (stat.isDirectory()) {
      const indexPath = join(target, 'index.html');
      try {
        if (statSync(indexPath).isFile()) return new Response(Bun.file(indexPath));
      } catch {
        // no index.html — fall through to a listing
      }
      return directoryListing(target, pathname);
    }

    return new Response(Bun.file(target));
  };
}

/** Bind an HTTP server for `root` on `port`. Throws on bind failure (e.g. port in use). */
export function startServer(root: string, port: number): ServeServer {
  return Bun.serve({ port, hostname: '0.0.0.0', fetch: createRequestHandler(root) });
}
