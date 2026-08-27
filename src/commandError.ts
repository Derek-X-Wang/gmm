import { invoke as tauriInvoke } from "@tauri-apps/api/core";

export type SurfaceFailureKind = "invalidActiveVariant" | "other";

export interface CommandErrorEnvelope {
  kind: SurfaceFailureKind;
  message: string;
}

/** A rejected Tauri command after its structured IPC envelope is normalized. */
export class CommandFailure extends Error {
  readonly kind: SurfaceFailureKind;

  constructor({ kind, message }: CommandErrorEnvelope) {
    super(message);
    this.name = "CommandFailure";
    this.kind = kind;
  }

  override toString(): string {
    return this.message;
  }
}

export function commandFailureFrom(error: unknown): CommandFailure {
  if (error instanceof CommandFailure) return error;
  if (
    typeof error === "object" &&
    error !== null &&
    typeof (error as { kind?: unknown }).kind === "string" &&
    typeof (error as { message?: unknown }).message === "string"
  ) {
    return new CommandFailure(error as CommandErrorEnvelope);
  }
  if (error instanceof Error) {
    return new CommandFailure({ kind: "other", message: error.message });
  }
  return new CommandFailure({ kind: "other", message: String(error) });
}

export function commandFailureMessage(error: unknown): string {
  return commandFailureFrom(error).message;
}

/** The only frontend entry point for fallible Tauri commands. */
export async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await tauriInvoke<T>(command, args);
  } catch (error) {
    throw commandFailureFrom(error);
  }
}
