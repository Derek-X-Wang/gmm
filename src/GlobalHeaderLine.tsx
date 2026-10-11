import type { ReactNode } from "react";

/** Attention and session facts describe all Games, independent of the selected tab. */
export function GlobalHeaderLine({ children }: { children: ReactNode }) {
  return <section className="global-header-line" aria-label="Global status">{children}</section>;
}
