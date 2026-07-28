// `package.json` is the single source of truth for the version. Bun inlines
// this JSON import into the compiled binary, so VERSION is fixed at build time
// without any codegen step. CI rewrites package.json (patch = run number)
// before building, so each push ships a strictly-increasing version.
import pkg from '../../package.json' with { type: 'json' };

export const VERSION: string = pkg.version;

/** `owner/repo` used for release checks and downloads. */
export const REPO = 'ohidurbappy/orb';
