// Used by every Windows MSI build, including the updater round trip.
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const workspace = fileURLToPath(new URL("../../src-tauri/", import.meta.url));
const target = process.env.TAURI_ENV_TARGET_TRIPLE;
if (!target?.endsWith("-windows-msvc")) {
  throw new Error("CLI sidecar packaging requires a Windows MSVC target from Tauri");
}
const profile = process.env.TAURI_ENV_DEBUG === "true" ? "debug" : "release";
const output = join(workspace, "binaries", `gmm-cli-${target}.exe`);
mkdirSync(join(workspace, "binaries"), { recursive: true });
// Never let a failed build silently package the last successful CLI.
rmSync(output, { force: true });

const config = JSON.parse(process.env.TAURI_CONFIG ?? "{}");
// gmm-cli depends on the app library. tauri-build copies externalBin even
// for a plain Cargo build, so bootstrap it without the not-yet-built sidecar.
config.bundle = { ...config.bundle, externalBin: [] };
const args = ["build", "--locked", "-p", "gmm-cli", "--target", target];
if (profile === "release") args.push("--release");
const build = spawnSync("cargo", args, {
  cwd: workspace,
  env: { ...process.env, TAURI_CONFIG: JSON.stringify(config) },
  stdio: "inherit",
});
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);
copyFileSync(join(workspace, "target", target, profile, "gmm-cli.exe"), output);
console.log(`Prepared ${output}`);
