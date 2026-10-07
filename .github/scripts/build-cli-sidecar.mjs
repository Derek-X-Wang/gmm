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
const rustc = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
if (rustc.error) throw rustc.error;
if (rustc.status !== 0) throw new Error("Could not read Rust host target");
const host = /^host: (.+)$/m.exec(rustc.stdout)?.[1];
if (!host) throw new Error("rustc did not report its host target");
// Tauri can rewrite the app package version for a bundle override (the
// updater test does this); let Cargo reconcile that workspace lock entry.
const args = ["build", "-p", "gmm-cli"];
// Reuse the native release dependencies already built by Tauri/CI.
if (target !== host) args.push("--target", target);
if (profile === "release") args.push("--release");
const build = spawnSync("cargo", args, {
  cwd: workspace,
  env: { ...process.env, TAURI_CONFIG: JSON.stringify(config) },
  stdio: "inherit",
});
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);
const metadata = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
  cwd: workspace,
  encoding: "utf8",
});
if (metadata.error) throw metadata.error;
if (metadata.status !== 0) throw new Error("Could not locate Cargo target directory");
const targetDir = JSON.parse(metadata.stdout).target_directory;
const binaryDir = target === host ? join(targetDir, profile) : join(targetDir, target, profile);
copyFileSync(join(binaryDir, "gmm-cli.exe"), output);
console.log(`Prepared ${output}`);
