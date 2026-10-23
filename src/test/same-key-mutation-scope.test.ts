// Query refreshes are not direct writes. Follow local callback bodies, reject
// opaque keys/options, and inventory all unresolved callback handoffs explicitly.
import ts from "typescript";
import { describe, expect, it } from "vitest";
import audit from "./ipc-boundary-audit.md?raw";
import { binding, checkInventory, hooks, inlineOptions, inventory, isFn, productionSources,
  property, resolve, unwrap, walk, type Hook, type Source } from "./ipc-boundary-source";

type Writer = Hook & { heads: Set<string>; scope?: string; opaque: Set<string>; errors: string[] };
function cacheHead(expression: ts.Expression | undefined): string | undefined {
  if (!expression) return;
  const key = resolve(expression);
  if (!ts.isArrayLiteralExpression(key) || !key.elements.length || key.elements.some(ts.isSpreadElement)) return;
  const head = resolve(key.elements[0]);
  return ts.isStringLiteral(head) || ts.isNumericLiteral(head) ? `${head.kind}:${head.text}` : undefined;
}
function mutationWriters(sources: Source[]): Writer[] {
  return sources.flatMap((source) => hooks(source, "useMutation")).map((site) => {
    const writer: Writer = { ...site, heads: new Set(), opaque: new Set(), errors: [] };
    if (!inlineOptions(site.options)) {
      writer.errors.push("opaque mutation options; use inline options without spreads/computed members");
      return writer;
    }
    const scope = property(site.options, "scope");
    const id = scope && inlineOptions(resolve(scope)) ? property(resolve(scope), "id") : undefined;
    if (id && ts.isStringLiteral(resolve(id))) writer.scope = (resolve(id) as ts.StringLiteral).text || undefined;
    const visited = new Set<ts.Node>();
    function inspect(callback: ts.Node) {
      if (visited.has(callback)) return;
      visited.add(callback);
      if (ts.isIdentifier(callback)) {
        const decl = binding(callback.text, callback);
        if (decl && ts.isFunctionDeclaration(decl)) inspect(decl);
        else if (decl && ts.isVariableDeclaration(decl) && decl.initializer && ts.isIdentifier(decl.name)) inspect(decl.initializer);
        else writer.opaque.add(callback.text);
        return;
      }
      if (!isFn(callback)) {
        writer.opaque.add(callback.getText());
        return;
      }
      if (!callback.body) { writer.opaque.add(callback.getText()); return; }
      walk(callback.body, (node) => {
        if (!ts.isCallExpression(node)) return;
        const callee = unwrap(node.expression);
        const method = ts.isPropertyAccessExpression(callee) ? callee.name.text :
          ts.isElementAccessExpression(callee) && callee.argumentExpression && ts.isStringLiteral(callee.argumentExpression)
            ? callee.argumentExpression.text : undefined;
        if (method === "setQueryData" || method === "setQueriesData") {
          const filters = node.arguments[0];
          const key = method === "setQueriesData"
            ? inlineOptions(filters) ? property(filters, "queryKey") : undefined
            : filters;
          const head = cacheHead(key);
          if (head) writer.heads.add(head);
          else writer.errors.push(`opaque cache key at ${node.getText()}; require a literal array head or an explained inventory exception before changing this gate`);
        } else if (method === "invalidateQueries" || method === "refetchQueries") {
          // Refreshes re-read the authoritative value; they do not install a
          // mutation's possibly stale return value into the cache.
        } else if (ts.isIdentifier(callee)) {
          const decl = binding(callee.text, callee);
          if (decl && ts.isFunctionDeclaration(decl)) inspect(decl);
          else if (decl && ts.isVariableDeclaration(decl) && decl.initializer && ts.isIdentifier(decl.name) &&
            (isFn(unwrap(decl.initializer)) || ts.isIdentifier(unwrap(decl.initializer)))) inspect(decl.initializer);
          else writer.opaque.add(callee.text);
        } else writer.opaque.add(callee.getText());
      });
    }
    for (const name of ["onSuccess", "onSettled"]) {
      const callback = property(site.options, name);
      if (callback) inspect(callback);
    }
    return writer;
  });
}
function scopeViolations(sources: Source[]): string[] {
  const writers = mutationWriters(sources);
  const errors = writers.flatMap((w) => w.errors.map((error) => `${w.location} ${w.key}: ${error}.`));
  const scopes = new Map<string, Set<string>>();
  for (const writer of writers) for (const head of writer.heads) {
    if (!writer.scope) errors.push(`${writer.location} ${writer.key}: cache writer for ${head} requires a literal shared scope id (including repeated calls to this site).`);
    const ids = scopes.get(head) ?? new Set<string>();
    ids.add(writer.scope ?? "<unscoped>");
    scopes.set(head, ids);
  }
  for (const [head, ids] of scopes) if (ids.size > 1)
    errors.push(`Cache key head ${head}: writers must share one scope id; found ${[...ids].join(", ")}.`);
  return errors;
}
function opaqueViolations(sources: Source[], audit: string): string[] {
  const writers = mutationWriters(sources);
  const errors = checkInventory("2", writers, audit);
  for (const writer of writers) {
    const expected = [...writer.opaque].sort().join(", ") || "none";
    const row = inventory(audit).find((r) => r.rule === "2" && r.key === writer.key);
    if (row?.opaque !== expected) errors.push(`${writer.key}: declare exact opaque callback handoffs "${expected}" with a written reason.`);
  }
  return errors;
}
const source = (body: string, path = "src/Fixture.tsx"): Source[] => [{ path, text: `function Widget() { ${body} }` }];
const write = (scope = '', key = '["modPanelWidth"]', callback = 'onSuccess') =>
  `{ ${scope} ${callback}: (_, value) => client.setQueryData(${key}, value) }`;

describe("same-key mutation scope boundary", () => {
  it("serializes direct cache writers in production source", () => {
    expect(scopeViolations(productionSources())).toEqual([]);
    expect(opaqueViolations(productionSources(), audit)).toEqual([]);
  });
  it("rejects an unscoped pair writing the same key", () => {
    expect(scopeViolations(source(`const a = useMutation(${write()}); const b = useMutation(${write()});`)))
      .toContainEqual(expect.stringContaining("requires a literal shared scope id"));
  });
  it("rejects the unscoped width writer from #432 even at a single site", () => {
    expect(scopeViolations(source(`const save = useMutation(${write()});`)))
      .toEqual([expect.stringContaining("requires a literal shared scope id")]);
  });
  it("accepts the fixed width scope and a matching cross-file pair", () => {
    const scoped = `const save = useMutation(${write('scope: { id: "modPanelWidth" },')});`;
    expect(scopeViolations([...source(scoped), ...source(scoped, "src/Other.tsx")])).toEqual([]);
  });
  it.each([
    ['scope: { id: "other" },', 'scope: { id: "modPanelWidth" },'],
    ['', 'scope: { id: "modPanelWidth" },'],
    ['scope: { id: dynamicId },', 'scope: { id: dynamicId },'],
  ])("rejects differing, missing or opaque shared scopes: %s / %s", (a, b) => {
    expect(scopeViolations(source(`const a = useMutation(${write(a)}); const b = useMutation(${write(b)});`)).length).toBeGreaterThan(0);
  });
  it("groups structural literal heads conservatively despite different dynamic suffixes", () => {
    expect(scopeViolations(source(`const a = useMutation(${write('scope: { id: "a" },', '["mods", game]')});
      const b = useMutation(${write('scope: { id: "b" },', '["mods", otherGame]', 'onSettled')});`)))
      .toContainEqual(expect.stringContaining("must share one scope id"));
  });
  it("resolves const keys, aliases, generics, namespace hooks and local callback helpers", () => {
    const sources = [{ path: "src/Fixture.tsx", text: `import { useMutation as mutate } from "@tanstack/react-query";
      const widthKey = ["modPanelWidth"] as const;
      function Widget() { const key = widthKey; const publish = () => qc.setQueryData(key, 320);
        const save = mutate<Result>({ scope: { id: "width" }, onSuccess: publish });
        const other = rq.useMutation({ scope: { id: "width" }, onSettled: () => publish() }); }` }];
    expect(mutationWriters(sources).map((w) => [...w.heads])).toEqual([[expect.stringContaining("modPanelWidth")], [expect.stringContaining("modPanelWidth")]]);
    expect(scopeViolations(sources)).toEqual([]);
  });
  it.each(['dynamicKey', '[dynamicHead, game]', '[...keys]', '[]'])
    ("fails closed on an opaque key: %s", (key) => {
      expect(scopeViolations(source(`const save = useMutation(${write('scope: { id: "x" },', key)});`)))
        .toContainEqual(expect.stringContaining("opaque cache key"));
    });
  it.each(['options', '{ ...options }', '{ onSuccess: publish, [option]: hidden }'])
    ("fails closed on opaque options: %s", (options) => {
      expect(scopeViolations(source(`const save = useMutation(${options});`)))
        .toContainEqual(expect.stringContaining("opaque mutation options"));
    });
  it("detects plural and bracket-notation direct writes", () => {
    expect(scopeViolations(source('const save = useMutation({ onSuccess: () => qc.setQueriesData({ queryKey: ["mods"] }, data) });')))
      .toContainEqual(expect.stringContaining("requires a literal shared scope id"));
    expect(scopeViolations(source('const save = useMutation({ onSettled: () => qc["setQueryData"](["mods"], data) });')))
      .toContainEqual(expect.stringContaining("requires a literal shared scope id"));
  });
  it("requires explained inventory rows and exact opaque callback handoffs", () => {
    const sources = source('const save = useMutation({ onSuccess: () => publish() });');
    expect(opaqueViolations(sources, "")).toContainEqual(expect.stringContaining("require exactly one explained"));
    expect(opaqueViolations(sources, '| 2 | Fixture / Widget::save | none | Reviewed. |'))
      .toContainEqual(expect.stringContaining('declare exact opaque callback handoffs "publish"'));
    const row = '| 2 | Fixture / Widget::save | publish | Parent callback handoff reviewed explicitly. |';
    expect(opaqueViolations(sources, row)).toEqual([]);
    expect(opaqueViolations(sources, row + '\n' + row)).toContainEqual(expect.stringContaining("exactly one explained"));
    expect(opaqueViolations(source('const save = useMutation({});'), row)).toContainEqual(expect.stringContaining('handoffs "none"'));
  });
  it("does not borrow local helper bodies from a sibling component", () => {
    const sources = [{ path: "src/Fixture.tsx", text: `function Other() { const publish = () => qc.setQueryData(["width"], 3); }
      function Widget() { const save = useMutation({ onSuccess: publish }); }` }];
    expect([...mutationWriters(sources)[0].opaque]).toEqual(["publish"]);
  });
  it("does not treat comments, strings or invalidation as direct cache writes", () => {
    expect(mutationWriters(source(`// useMutation({ onSuccess: () => qc.setQueryData(["ignored"], 1) })
      const text = 'setQueryData'; const save = useMutation({ onSettled: () => qc.invalidateQueries({ queryKey: [dynamic] }) });`))[0].heads.size).toBe(0);
  });
});
