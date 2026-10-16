// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

// These are authored-CSS guards. jsdom cannot prove viewport geometry or
// native Windows rendering; the sweep records those separately as NOT-RUN.
const css: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
function declarations(selector: string): Record<string, string> {
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selectors]) => selectors.split(",").some((part) => part.trim() === selector));
  expect(rules.length, `authored rule for ${selector}`).toBeGreaterThan(0);
  return Object.fromEntries(rules.flatMap(([, , body]) => body.split(";")
    .filter((part) => part.includes(":"))
    .map((part) => part.split(":").map((value) => value.trim()))));
}

function luminance(hex: string): number {
  expect(hex, "contrast inputs must be opaque authored colours").toMatch(/^#[0-9a-f]{6}$/i);
  const linear = [1, 3, 5].map((start) => {
    const channel = parseInt(hex.slice(start, start + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
}

it("keeps main hierarchy, status and attention containers inheriting the dark text colour", () => {
  for (const selector of [".game-header h1", ".global-header-line", ".main-screen__notices",
    ".game-tabs", ".game-status-strip", ".game-status-strip__summary", ".attention-indicator",
    ".attention-indicator__report"]) {
    expect(declarations(selector), `${selector} inherits its surface text`).not.toHaveProperty("color");
  }
});

it("keeps normal text in the hierarchy, status strip and both attention states at readable dark contrast", () => {
  const root = declarations(":root");
  const launch = declarations(".game-header__launch");
  const active = declarations(".game-tabs__tab--active");
  const inactive = declarations(".game-tabs__tab");
  const pairs = [
    ["Game title and quiet attention", root.color, root.background],
    ["status summary", root.color, declarations(".status-bar").background],
    ["muted status and recovery context", declarations(".muted").color, root.background],
    ["attention scope", declarations(".attention-indicator__scope").color, root.background],
    ["loud attention", declarations(".attention-indicator__toggle--loud").color, root.background],
    ["inactive Game tab", inactive.color, inactive.background],
    ["selected Game tab", active.color === "white" ? "#ffffff" : active.color, active.background],
    ["Launch", launch.color === "white" ? "#ffffff" : launch.color, launch.background],
  ];
  for (const [name, foreground, background] of pairs) {
    const values = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
    expect((values[0] + 0.05) / (values[1] + 0.05), `${name}: normal text contrast >= 4.5:1`).toBeGreaterThanOrEqual(4.5);
  }
});

it("preserves authored wrapping for the hierarchy, status, recorded paths and Settings forms", () => {
  for (const selector of [".row", ".game-tabs", ".game-status-strip", ".attention-indicator__controls"]) {
    expect(declarations(selector)["flex-wrap"], `${selector} wraps controls`).toBe("wrap");
  }
  for (const selector of [".game-header h1", ".game-status-strip__summary", ".settings-content p",
    ".reinstall-recovery-warning dd", ".library-audit-warning code", ".duplicate-mod-details dd"]) {
    expect(declarations(selector)["overflow-wrap"], `${selector} wraps long evidence`).toBe("anywhere");
  }
  expect(declarations(".settings-content")["min-width"]).toBe("0");
  expect(declarations(".settings-content input.path")).toMatchObject({ "min-width": "0", width: "100%" });
});
