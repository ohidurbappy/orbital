# CLAUDE.md

Conventions for working in this repo. Keep it clean and consistent.

## What this is

`orb` is a single cross-platform CLI binary that aggregates many small tools. It is
built with Ink (React + TypeScript) and compiled to standalone binaries with Bun.

## Architecture

- **Command registry** (`src/commands/index.ts`) is the single source of truth.
  `COMMANDS` drives `--help`, the interactive menu, and CLI dispatch. Register new
  commands there.
- **Folder per command** under `src/commands/<name>/`:
  - `<name>.ts` — pure logic. **Inject side-effecting dependencies as parameters**
    (the `os` functions, `fetch`, file reads) with real defaults, so tests pass
    fixtures instead of mocking modules. See `ip.ts` and `sysinfo.ts`.
  - `<Name>Command.tsx` — a thin Ink component; no business logic.
  - `index.ts` — exports a `Command` descriptor (`src/commands/types.ts`).
  - tests alongside.
- **UI is separated from logic.** Components render; they don't compute. Shared UI
  lives in `src/components/` (e.g. `KeyValue`).
- **Entry/dispatch:** `src/cli.tsx` parses argv and renders `src/app.tsx`. `App`
  shows the `UpdateBanner` plus either a command (one-shot) or the `Menu`.
- **Auto-update:** `src/core/updater/`. `checkForUpdate` queries GitHub releases;
  `state.ts` caches results (10-min throttle) at `~/.config/orb/state.json`;
  `useUpdateCheck` polls while the menu is open; one-shot commands fire a detached
  background refresh (`spawnBackgroundRefresh`) and never block on the network;
  `applyUpdate` self-replaces the binary for `orb update`.

## Command lifecycle

- Simple print-and-exit commands (`ip`, `sysinfo`) leave `managesExit` unset — `App`
  exits automatically after the first frame.
- Commands that do async work or stay interactive set `managesExit: true` and call
  `useApp().exit()` themselves (see `update`).

## Conventions

- TypeScript strict mode; `bun run typecheck` must pass.
- JSX uses the automatic runtime — **do not** `import React` just for JSX.
- Errors in update/network paths are swallowed into safe results — they must never
  break the command the user actually ran.
- Asset names in `src/core/updater/assets.ts` must match the output names in
  `scripts/build.ts` and the files uploaded by `.github/workflows/release.yml`.
  Release assets are **gzipped** (`<name>.gz`); `scripts/build.ts` gzips each
  compiled binary and `applyUpdate` gunzips on download.

## Commands

```sh
bun test            # tests (bun:test + ink-testing-library)
bun run typecheck   # tsc --noEmit
bun run dev <args>  # run from source
bun run build       # cross-compile all targets
```

## Release

**Every push to `main` cuts a release** (`.github/workflows/release.yml`): it runs
typecheck + tests, cross-compiles all targets on one runner, gzips them, generates
`checksums.txt`, and creates the GitHub release.

Versioning is automatic. `package.json`'s `version` is the single source of truth
and is inlined into the binary at build time (`src/core/version.ts`). CI takes its
`MAJOR.MINOR` and uses the workflow run number as the patch (`MAJOR.MINOR.<run>`),
so every push ships a strictly-increasing semver — no manual bump needed. To start
a new major/minor series, edit `version` in `package.json`.
