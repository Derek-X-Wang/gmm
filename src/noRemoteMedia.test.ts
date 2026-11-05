// Repo-wide guard (#437): nothing under src/ may make the WebView load a remote
// image, frame or video. Reads source, like the stylesheet guards in App.test.tsx.
//
// Why: a WebView load goes straight to the network. It cannot use the proxy the
// user configured (network.proxy.*), so behind a blocked network the image just
// fails with nothing to inspect (release builds have no devtools, #435), and on
// any network it discloses the user's IP address and which Mod they are looking
// at, despite the proxy. The Core fetches the bytes instead (get_mod_screenshot)
// and the only permitted image source is the data URI it returns.
// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const WHY = [
  "A WebView load cannot use the proxy configured in Settings (network.proxy.*).",
  "Behind a blocked network the media silently fails; on any network it discloses the user's IP",
  "address and which Mod they are viewing, despite the proxy.",
  "Fetch the bytes in the Core with Core::http_client (see get_mod_screenshot) and render the",
  "data URI it returns (`src={shot.dataUri}`) instead.",
].join(" ");

/** Elements that can fetch media. Only `<img>` is ever allowed, and only from a data URI. */
const FORBIDDEN_TAGS = ["iframe", "video", "audio", "embed", "object", "source", "track", "picture"];

function stripCssComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "");
}

/** Blank comments out of TS/TSX without touching `//` inside strings such as "https://x". */
function stripCodeComments(text: string): string {
  let out = "";
  for (let i = 0; i < text.length;) {
    const c = text[i];
    const next = text[i + 1];
    if (c === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end === -1 ? text.length : end + 2;
    } else if (c === "/" && next === "/") {
      while (i < text.length && text[i] !== "\n") i++;
    } else if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < text.length && text[j] !== c) j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

type Attribute = { name: string; kind: "literal" | "expression" | "none"; value: string };

/** The text of the JSX opening tag starting at `start` (`<`), brace- and quote-aware. */
function openingTag(text: string, start: number): string {
  let depth = 0;
  let quote = "";
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = "";
    } else if (c === '"' || c === "'" || c === "`") quote = c;
    else if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === ">" && depth === 0) return text.slice(start, i + 1);
  }
  return text.slice(start);
}

function attributes(tag: string): Attribute[] {
  const found: Attribute[] = [];
  let i = tag.search(/[\s/]/);
  while (i >= 0 && i < tag.length) {
    while (i < tag.length && /[\s/>]/.test(tag[i])) i++;
    if (i >= tag.length) break;
    if (tag[i] === "{") {
      const end = balanced(tag, i);
      found.push({ name: "...spread", kind: "expression", value: tag.slice(i + 1, end) });
      i = end + 1;
      continue;
    }
    const name = /^[\w:.-]+/.exec(tag.slice(i))?.[0];
    if (!name) { i++; continue; }
    i += name.length;
    while (i < tag.length && /\s/.test(tag[i])) i++;
    if (tag[i] !== "=") { found.push({ name, kind: "none", value: "" }); continue; }
    i++;
    while (i < tag.length && /\s/.test(tag[i])) i++;
    if (tag[i] === '"' || tag[i] === "'") {
      const end = tag.indexOf(tag[i], i + 1);
      found.push({ name, kind: "literal", value: tag.slice(i + 1, end) });
      i = end + 1;
    } else if (tag[i] === "{") {
      const end = balanced(tag, i);
      found.push({ name, kind: "expression", value: tag.slice(i + 1, end).trim() });
      i = end + 1;
    }
  }
  return found;
}

function balanced(text: string, open: number): number {
  let depth = 0;
  let quote = "";
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = "";
    } else if (c === '"' || c === "'" || c === "`") quote = c;
    else if (c === "{") depth++;
    else if (c === "}" && --depth === 0) return i;
  }
  return text.length - 1;
}

/** `shot.dataUri`, `data.dataUri`, `dataUri`: a field named for what the Core hands over. */
const DATA_URI_EXPRESSION = /^[A-Za-z_$][\w$]*(?:\??\.[A-Za-z_$][\w$]*)*$/;
const isCoreDataUri = (expression: string) =>
  DATA_URI_EXPRESSION.test(expression) && /(?:^|\.)dataUri$/.test(expression);

function imgViolation(tag: string): string | null {
  const attrs = attributes(tag);
  if (attrs.some((a) => a.name === "...spread")) return "spread props could carry a remote src";
  if (attrs.some((a) => /^srcset$/i.test(a.name))) return "srcSet is not allowed";
  const src = attrs.find((a) => a.name === "src");
  if (!src) return "no src the guard can verify";
  if (src.kind === "literal") return src.value.startsWith("data:image/") ? null : `src is "${src.value}"`;
  if (src.kind === "expression" && isCoreDataUri(src.value)) return null;
  return `src={${src.value}} is not the Core-supplied data URI`;
}

/** Every remote-media reference in one source file, as short human-readable lines. */
export function remoteMediaViolations(path: string, raw: string): string[] {
  const isCss = path.endsWith(".css");
  const text = isCss ? stripCssComments(raw) : stripCodeComments(raw);
  const found: string[] = [];
  const add = (what: string) => found.push(`${path}: ${what}`);

  if (!isCss) {
    for (const match of text.matchAll(/<img\b/g)) {
      const why = imgViolation(openingTag(text, match.index));
      if (why) add(`<img> ${why}`);
    }
    for (const tag of FORBIDDEN_TAGS)
      if (new RegExp(`<${tag}\\b`, "i").test(text)) add(`<${tag}> can load remote media`);
    if (/\bnew\s+Image\s*\(/.test(text)) add("new Image() loads outside the proxy");
    if (/createElement\(\s*["'`](?:img|iframe|video|audio|embed|object|source|link|script)["'`]/i.test(text))
      add("createElement of a media element");
    if (/\.src\s*=(?!=)/.test(text)) add("assigning .src");
  }
  // Applies to CSS and to style strings in code alike.
  for (const match of text.matchAll(/(?<![\w$-])url\(\s*(?!["']?(?:data:image\/|#))[^)]*\)/gi))
    add(`${match[0]} points a style at something other than a data:image URI`);
  if (/image-set\(/i.test(text)) add("image-set() can reference remote images");
  if (/@import\b/i.test(text)) add("@import can pull in remote CSS");
  return found;
}

describe("remote media guard", () => {
  it("finds no remote image, frame or video reference anywhere under src/", () => {
    const files: string[] = [];
    const visit = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true }) as { name: string; isDirectory(): boolean }[]) {
        const path = `${dir}/${entry.name}`;
        if (entry.isDirectory()) visit(path);
        else if (/\.(?:tsx?|css)$/.test(entry.name) && !/\.(?:test|spec)\.tsx?$/.test(entry.name)) files.push(path);
      }
    };
    visit("src");
    expect(files.length, "the walk found the sources").toBeGreaterThan(20);

    const violations = files.flatMap((path) => remoteMediaViolations(path, readFileSync(path, "utf8") as string));
    expect(violations, WHY).toEqual([]);
  });

  describe("the scanner itself", () => {
    const bad = (code: string, path = "src/Fixture.tsx") => remoteMediaViolations(path, code);
    it.each([
      '<img src="https://images.gamebanana.com/a.png" />',
      "<img src='http://x.test/a.png' alt='' />",
      '<img src="//x.test/a.png" />',
      '<img src={mod.screenshotUrl ?? ""} />',
      "<img src={mod.screenshotUrl} alt={`Screenshot of ${mod.name}`} />",
      "<img src={`${base}/a.png`} />",
      '<img src={url} srcSet="a.png 2x" />',
      "<img {...props} />",
      "<img alt='no source' />",
      '<img src="file:///etc/passwd" />',
      '<iframe src="https://x.test" />',
      "<video src={clip} />",
      "<audio src={clip} />",
      "<embed src={x} />",
      "<object data={x} />",
      "<source srcSet={x} />",
      'const i = new Image(); i.onload = f;',
      'document.createElement("img");',
      "image.src = remote;",
    ])("rejects %s", (code) => {
      expect(bad(code)).not.toEqual([]);
    });
    it.each([
      'a { background-image: url(https://x.test/a.png); }',
      'a { background: url("http://x.test/a.png"); }',
      "a { background: url(//x.test/a.png) }",
      "a { background: url(/relative.png) }",
      "a { background-image: image-set(url(a.png) 1x); }",
      '@import "https://x.test/a.css";',
    ])("rejects stylesheet reference %s", (css) => {
      expect(bad(css, "src/Fixture.css")).not.toEqual([]);
    });
    it("does not mistake a function named like url() for a style", () => {
      expect(bad("setProxyUrl(cfg.url ?? ''); parseUrl(x);")).toEqual([]);
    });
    it("rejects a style string in code", () => {
      expect(bad('const s = { backgroundImage: "url(https://x.test/a.png)" };')).not.toEqual([]);
    });
    it("finds an img whose attributes span lines and contain arrow functions", () => {
      expect(bad(`<img
        onLoad={() => setLoaded(a > b)}
        src={remote}
      />`)).not.toEqual([]);
    });
    it.each([
      "<img src={screenshot.data.dataUri} alt='x' />",
      "<img src={screenshot.data?.dataUri} alt={`Screenshot of ${mod.name}`} />",
      "<img src={dataUri} alt='' onLoad={() => ok(a > b)} />",
      '<img src="data:image/png;base64,AAAA" alt="" />',
    ])("allows the Core-supplied data URI: %s", (code) => {
      expect(bad(code)).toEqual([]);
    });
    it("ignores comments, links to remote pages and https text in strings", () => {
      expect(bad("// <img src=\"https://x.test/a.png\" />\n/* <iframe /> */\nconst a = 1;")).toEqual([]);
      expect(bad('<a href="https://gamebanana.com/mods/1" target="_blank">View</a>')).toEqual([]);
      expect(bad('const home = "https://gamebanana.com"; <p>{home}</p>')).toEqual([]);
      expect(bad("/* url(https://x.test/a.png) */ a { color: red }", "src/Fixture.css")).toEqual([]);
      expect(bad("a { background: url(data:image/png;base64,AAAA) }", "src/Fixture.css")).toEqual([]);
    });
  });
});
