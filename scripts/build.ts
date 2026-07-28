#!/usr/bin/env bun
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

interface Target {
  /** Bun cross-compile target triple. */
  bun: string;
  /** Output filename — must match `assetNameFor` in src/core/updater/assets.ts. */
  out: string;
}

const TARGETS: Target[] = [
  { bun: 'bun-darwin-arm64', out: 'orb-darwin-arm64' },
  { bun: 'bun-darwin-x64', out: 'orb-darwin-x64' },
  { bun: 'bun-linux-x64', out: 'orb-linux-x64' },
  { bun: 'bun-linux-arm64', out: 'orb-linux-arm64' },
  { bun: 'bun-windows-x64', out: 'orb-windows-x64.exe' },
];

mkdirSync('dist', { recursive: true });

// Allow building a single target: `bun run scripts/build.ts darwin-arm64`
const filter = process.argv[2];
const targets = filter ? TARGETS.filter((t) => t.out.includes(filter)) : TARGETS;

let failures = 0;
for (const target of targets) {
  console.log(`▶ building ${target.out} (${target.bun})`);
  const result = spawnSync(
    'bun',
    [
      'build',
      'src/cli.tsx',
      '--compile',
      // Minify the bundled JS before it's embedded in the binary. This is the
      // only safe size/startup lever here: `--bytecode` can't compile a
      // dependency (yoga-layout), and forcing React into production mode
      // (NODE_ENV=production) breaks `react/jsx-runtime` resolution under
      // `--compile`, producing a binary that throws at startup.
      '--minify',
      `--target=${target.bun}`,
      `--outfile=dist/${target.out}`,
    ],
    { stdio: 'inherit' },
  );
  if (result.status !== 0) {
    console.error(`✗ failed: ${target.out}`);
    failures++;
    continue;
  }

  // Ship gzipped binaries: ~62% smaller downloads. The on-disk binary is
  // unchanged after the updater gunzips it (see applyUpdate.ts). The asset
  // name must match `assetNameFor` in src/core/updater/assets.ts.
  const binPath = `dist/${target.out}`;
  const gzPath = `${binPath}.gz`;
  writeFileSync(gzPath, gzipSync(readFileSync(binPath), { level: 9 }));
  const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
  console.log(`  ↳ gzip ${target.out}.gz (${mb(statSync(binPath).size)} → ${mb(statSync(gzPath).size)} MB)`);
}

if (failures > 0) {
  console.error(`\n${failures} target(s) failed.`);
  process.exit(1);
}
console.log(`\n✓ built ${targets.length} target(s) into dist/`);
