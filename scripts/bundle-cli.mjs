#!/usr/bin/env node
// Materialize the @synapseia-network/node CLI into
// src-tauri/resources/cli-bundle.tar.gz so the Tauri bundle ships a
// fallback copy that works on a clean machine with no npm / no
// network.
//
// Single-bundle layout (post 0.8.20):
//   The CLI ships as a tsup-bundled `dist/index.js` (~14MB) with
//   every JS dep inlined. The only required `node_modules` entries
//   are the native usearch chain — `usearch` itself plus its
//   load-time deps (`bindings`, `node-gyp-build`, `node-addon-api`,
//   `file-uri-to-path`). Total ~70MB vs the old ~365MB tree (~5x
//   smaller DMG).
//
// Two modes:
//   1. SYNAPSEIA_BUNDLE_FROM_WORKSPACE (default for local dev):
//      copy from ../node in the monorepo. Assumes its dist/ and
//      node_modules/ exist (run `pnpm --filter @synapseia-network/node
//      build` + `pnpm install` first). pnpm's symlinked layout means
//      we MUST dereference when copying so the bundle is
//      self-contained.
//   2. SYNAPSEIA_BUNDLE_FROM_NPM (CI on the standalone node-ui
//      repo): npm-install the published CLI into a scratch dir and
//      copy from there. CI does this directly in release.yml; this
//      branch is here for parity.
//
// Idempotent: removes the existing tarball first.

import {
  existsSync,
  mkdirSync,
  rmSync,
  cpSync,
  writeFileSync,
  readFileSync,
  readdirSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const NODE_UI_ROOT = resolve(__dirname, "..");
const RESOURCES_DIR = join(NODE_UI_ROOT, "src-tauri", "resources");
const STAGE = join(RESOURCES_DIR, ".cli-bundle-stage");
const TARBALL = join(RESOURCES_DIR, "cli-bundle.tar.gz");

// Native runtime deps of `usearch` we must ship alongside it. The
// CLI bundle externalizes `usearch` (it loads `.node` binaries via
// dlopen and can't be inlined); usearch in turn does
// `import 'bindings'` / `import 'node-gyp-build'` at load time.
// Missing any of these makes `start` crash with
// `Cannot find package 'node-gyp-build'` the first time
// `KgShardHnswSearcher` initializes.
const NATIVE_DEPS = [
  "usearch",
  "bindings",
  "node-gyp-build",
  "node-addon-api",
  "file-uri-to-path",
];

function rmrf(p) {
  if (existsSync(p)) rmSync(p, { recursive: true, force: true });
}

/**
 * Copy `<srcRoot>/node_modules/<name>` to `<destRoot>/node_modules/<name>`.
 * Dereferences symlinks so the result is self-contained (pnpm uses
 * symlinks heavily, npm hoisting can too). Returns true if the
 * source existed.
 */
function copyNodeModule(srcRoot, destRoot, name) {
  const src = join(srcRoot, "node_modules", name);
  if (!existsSync(src)) return false;
  const dest = join(destRoot, "node_modules", name);
  cpSync(src, dest, { recursive: true, dereference: true });
  return true;
}

function bundleFromWorkspace() {
  const nodePkg = resolve(NODE_UI_ROOT, "..", "node");
  const distSrc = join(nodePkg, "dist");
  const pkgJsonSrc = join(nodePkg, "package.json");

  if (!existsSync(distSrc) || !existsSync(pkgJsonSrc)) {
    console.error(
      `[bundle-cli] ${distSrc} or ${pkgJsonSrc} missing. Build the node package first (pnpm --filter @synapseia-network/node build).`,
    );
    process.exit(1);
  }

  rmrf(STAGE);
  mkdirSync(STAGE, { recursive: true });
  mkdirSync(join(STAGE, "node_modules"), { recursive: true });

  // 1. The bundled CLI.
  cpSync(distSrc, join(STAGE, "dist"), { recursive: true });
  cpSync(pkgJsonSrc, join(STAGE, "package.json"));

  // 2. Native deps. pnpm hoists most of these to packages/node/
  //    node_modules/ directly (visible to the CLI's own resolution).
  //    Missing siblings (e.g. when pnpm nests inside .pnpm/) get
  //    walked manually from the .pnpm store.
  for (const dep of NATIVE_DEPS) {
    if (copyNodeModule(nodePkg, STAGE, dep)) {
      continue;
    }
    // pnpm fallback: scan .pnpm/ for the latest match.
    const pnpmStore = join(nodePkg, "node_modules", ".pnpm");
    if (!existsSync(pnpmStore)) {
      console.error(
        `[bundle-cli] native dep '${dep}' not found at ${nodePkg}/node_modules/${dep} and no .pnpm/ store to fall back on.`,
      );
      process.exit(1);
    }
    const matches = readdirSync(pnpmStore)
      .filter((d) => d.startsWith(`${dep}@`))
      .sort();
    if (matches.length === 0) {
      console.error(
        `[bundle-cli] native dep '${dep}' not found in .pnpm/ store either.`,
      );
      process.exit(1);
    }
    const chosen = matches[matches.length - 1];
    const src = join(pnpmStore, chosen, "node_modules", dep);
    if (!existsSync(src)) {
      console.error(`[bundle-cli] expected ${src} from .pnpm/ match`);
      process.exit(1);
    }
    cpSync(src, join(STAGE, "node_modules", dep), {
      recursive: true,
      dereference: true,
    });
  }
}

function bundleFromNpm() {
  // Pin to OUR own version so we never install a newer/older CLI
  // than the desktop release. node-ui and @synapseia-network/node
  // ship in lockstep (Version sync rule).
  const ownVersion =
    process.env.npm_package_version ??
    JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8"),
    ).version;
  const npmStage = join(NODE_UI_ROOT, ".cli-npm-stage");
  rmrf(npmStage);
  mkdirSync(npmStage, { recursive: true });
  writeFileSync(
    join(npmStage, "package.json"),
    JSON.stringify(
      {
        name: "syn-cli-bundle-stage",
        version: "0.0.0",
        private: true,
        overrides: { "rpc-websockets": { uuid: "8.3.2" } },
        dependencies: { "@synapseia-network/node": ownVersion },
      },
      null,
      2,
    ),
  );
  // --ignore-scripts so the CLI's postinstall (patch-package) doesn't
  // run with cwd = installed-package dir. We re-run patch-package
  // manually below from the scratch root, where the hoisted layout
  // is visible. Once the published CLI's `dist/index.js` is itself
  // pre-bundled with the patched libp2p source inlined, the patches
  // step becomes a no-op — but keep it until we explicitly drop the
  // patches/ dir from the published tarball.
  execSync(
    "npm install --omit=dev --no-audit --no-fund --ignore-scripts",
    { cwd: npmStage, stdio: "inherit" },
  );
  const cliDir = join(npmStage, "node_modules", "@synapseia-network", "node");
  const patchesDir = join(cliDir, "patches");
  if (
    existsSync(patchesDir) &&
    readdirSync(patchesDir).filter((f) => f.endsWith(".patch")).length > 0
  ) {
    const relPatchDir = join(
      "node_modules",
      "@synapseia-network",
      "node",
      "patches",
    );
    execSync(
      `npx --yes patch-package@8 --patch-dir "${relPatchDir}" --error-on-fail`,
      { cwd: npmStage, stdio: "inherit" },
    );
  } else {
    console.log("[bundle-cli] no patches to apply (empty or missing)");
  }

  rmrf(STAGE);
  mkdirSync(STAGE, { recursive: true });
  mkdirSync(join(STAGE, "node_modules"), { recursive: true });

  // 1. The bundled CLI.
  cpSync(join(cliDir, "dist"), join(STAGE, "dist"), { recursive: true });
  cpSync(join(cliDir, "package.json"), join(STAGE, "package.json"));

  // 2. Native deps. npm hoists these to npmStage/node_modules/.
  for (const dep of NATIVE_DEPS) {
    const src = join(npmStage, "node_modules", dep);
    if (!existsSync(src)) {
      console.error(
        `[bundle-cli] native dep '${dep}' missing at ${src} after npm install`,
      );
      process.exit(1);
    }
    cpSync(src, join(STAGE, "node_modules", dep), {
      recursive: true,
      dereference: true,
    });
  }
  rmrf(npmStage);
}

function tarballAndCleanup() {
  // Wrap the entire materialized CLI into ONE opaque .tar.gz so the
  // Tauri bundler (linuxdeploy in particular) treats it as plain
  // data and doesn't walk thousands of files. Without this, Linux
  // AppImage builds bailed silently. With the new bundle the file
  // count is much lower but the tarball indirection is still
  // valuable (single resource entry, deterministic extraction
  // checkpoint via cli-bundle.tar.gz size).
  if (!existsSync(STAGE)) {
    console.error(`[bundle-cli] expected ${STAGE} to exist before tarballing`);
    process.exit(1);
  }
  rmrf(TARBALL);
  console.log("[bundle-cli] stage size pre-tar:");
  execSync(`du -sh "${STAGE}"`, { stdio: "inherit" });
  // `-C stage .` packs the contents (not the stage/ dir itself) so
  // extraction at runtime produces dist/, node_modules/,
  // package.json directly in the target dir.
  // Use a RELATIVE output path so Windows tar doesn't parse the
  // leading drive letter (`D:\...`) as rcp host:path. macOS BSD tar
  // doesn't support --force-local, so a relative path is the
  // portable fix.
  const tarballDir = dirname(TARBALL);
  const tarballName = TARBALL.slice(tarballDir.length + 1);
  execSync(`tar -czf "${tarballName}" -C "${STAGE}" .`, {
    cwd: tarballDir,
    stdio: "inherit",
  });
  rmrf(STAGE);
  console.log("[bundle-cli] tarball size:");
  execSync(`du -sh "${TARBALL}"`, { stdio: "inherit" });
}

const fromNpm = process.env.SYNAPSEIA_BUNDLE_FROM_NPM === "1";
if (fromNpm) {
  console.log("[bundle-cli] mode=npm");
  bundleFromNpm();
} else {
  console.log("[bundle-cli] mode=workspace");
  bundleFromWorkspace();
}
tarballAndCleanup();
console.log(`[bundle-cli] tarball at ${TARBALL}`);
