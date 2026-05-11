#!/usr/bin/env node
// Materialize the @synapseia-network/node CLI into
// src-tauri/resources/cli so the Tauri bundle ships a fallback copy that
// works on a clean machine with no npm / no network.
//
// Two modes:
//   1. SYNAPSEIA_BUNDLE_FROM_WORKSPACE=1 (default for local dev): copy
//      from ../node in the monorepo. Assumes that package's dist + prod
//      node_modules exist (run `pnpm --filter @synapseia-network/node
//      build` and `pnpm install --prod` first).
//   2. SYNAPSEIA_BUNDLE_FROM_NPM=1 (CI on the standalone node-ui repo):
//      npm-install the published CLI into a scratch dir and copy from
//      there. CI does this directly in release.yml; this branch is here
//      for parity if a dev wants to test that path.
//
// Idempotent: removes the existing materialized cli/ first.

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
const DEST = join(NODE_UI_ROOT, "src-tauri", "resources", "cli");

function rmrf(p) {
  if (existsSync(p)) rmSync(p, { recursive: true, force: true });
}

function ensureKeep(path) {
  // Re-create the .gitkeep so the empty placeholder directories that
  // unblock `cargo check` after a clean still exist if no copy happened.
  if (!existsSync(path)) mkdirSync(path, { recursive: true });
  writeFileSync(join(path, ".gitkeep"), "");
}

function bundleFromWorkspace() {
  const nodePkg = resolve(NODE_UI_ROOT, "..", "node");
  const distSrc = join(nodePkg, "dist");
  const pkgJsonSrc = join(nodePkg, "package.json");
  const nodeModulesSrc = join(nodePkg, "node_modules");
  const patchesSrc = join(nodePkg, "patches");

  if (!existsSync(distSrc) || !existsSync(pkgJsonSrc)) {
    console.error(
      `[bundle-cli] ${distSrc} or ${pkgJsonSrc} missing. Build the node package first.`,
    );
    process.exit(1);
  }

  rmrf(DEST);
  mkdirSync(DEST, { recursive: true });

  cpSync(distSrc, join(DEST, "dist"), { recursive: true });
  cpSync(pkgJsonSrc, join(DEST, "package.json"));
  if (existsSync(nodeModulesSrc)) {
    cpSync(nodeModulesSrc, join(DEST, "node_modules"), {
      recursive: true,
      // dereference symlinks so the bundled tree is self-contained
      // (workspace installs use symlinks heavily for hoisted deps).
      dereference: true,
    });
  } else {
    mkdirSync(join(DEST, "node_modules"), { recursive: true });
    ensureKeep(join(DEST, "node_modules"));
  }
  if (existsSync(patchesSrc)) {
    cpSync(patchesSrc, join(DEST, "patches"), { recursive: true });
  } else {
    ensureKeep(join(DEST, "patches"));
  }
}

function bundleFromNpm() {
  // Pin to OUR own version so we never install a newer/older CLI than the
  // desktop release. node-ui and @synapseia-network/node ship in lockstep
  // (Version sync rule); mismatches between bundled CLI and app shipped to
  // users are the exact race this guards against.
  const ownVersion =
    process.env.npm_package_version ??
    JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8"),
    ).version;
  const stage = join(NODE_UI_ROOT, ".cli-stage");
  rmrf(stage);
  mkdirSync(stage, { recursive: true });
  execSync("npm init -y", { cwd: stage, stdio: "ignore" });
  // Use --ignore-scripts so the CLI's postinstall (patch-package)
  // doesn't run with cwd = installed-package dir. npm hoists
  // @libp2p/utils to the scratch root's node_modules (not nested under
  // the CLI), and patch-package run from the package dir can't see the
  // hoisted dep. We re-run patch-package manually below from the
  // scratch root, where the hoisted layout is visible.
  execSync(
    `npm install --omit=dev --no-audit --no-fund --ignore-scripts @synapseia-network/node@${ownVersion}`,
    { cwd: stage, stdio: "inherit" },
  );
  const cliDir = join(stage, "node_modules", "@synapseia-network", "node");
  // Apply patches from the scratch root so patch-package can resolve
  // hoisted deps. patch-dir points at the installed package's patches/.
  const patchesDir = join(cliDir, "patches");
  if (
    existsSync(patchesDir) &&
    readdirSync(patchesDir).filter((f) => f.endsWith(".patch")).length > 0
  ) {
    const relPatchDir = join("node_modules", "@synapseia-network", "node", "patches");
    execSync(
      `npx --yes patch-package@8 --patch-dir "${relPatchDir}" --error-on-fail`,
      { cwd: stage, stdio: "inherit" },
    );
  } else {
    console.log("[bundle-cli] no patches to apply (empty or missing)");
  }
  rmrf(DEST);
  mkdirSync(DEST, { recursive: true });
  cpSync(join(cliDir, "dist"), join(DEST, "dist"), { recursive: true });
  cpSync(join(cliDir, "package.json"), join(DEST, "package.json"));
  cpSync(join(stage, "node_modules"), join(DEST, "node_modules"), {
    recursive: true,
    dereference: true,
  });
  if (existsSync(join(cliDir, "patches"))) {
    cpSync(join(cliDir, "patches"), join(DEST, "patches"), { recursive: true });
  } else {
    ensureKeep(join(DEST, "patches"));
  }
  rmrf(stage);
}

const fromNpm = process.env.SYNAPSEIA_BUNDLE_FROM_NPM === "1";
if (fromNpm) {
  console.log("[bundle-cli] mode=npm");
  bundleFromNpm();
} else {
  console.log("[bundle-cli] mode=workspace");
  bundleFromWorkspace();
}
console.log(`[bundle-cli] bundled into ${DEST}`);
