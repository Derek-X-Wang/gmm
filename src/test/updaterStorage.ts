import type { SkipStorage } from "../updater";

/** A durable store shared across simulated controllers, without Tauri IPC. */
export function memorySkipStorage(): SkipStorage {
  const values = new Map<string, string>();
  return { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); } };
}
export function signedMetadata(version: string) {
  return { url: `https://example.com/${version}.msi`, signature: `signed-${version}` };
}
