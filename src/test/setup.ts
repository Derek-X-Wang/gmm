import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// Every component test gets a fresh DOM.
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

// jsdom has no matchMedia; some React libs probe for it.
Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }),
});

// jsdom does not implement scrolling. Individual return tests inspect this seam.
Object.defineProperty(window, "scrollTo", { writable: true, value: vi.fn() });
