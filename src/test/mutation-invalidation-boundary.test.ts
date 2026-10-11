// Structural gate for recorded frontend invalidation decisions, not their correctness.
// Parse production TS/TSX, including new files, with the already-installed TypeScript
// parser. Recognize direct/aliased/namespace useMutation calls with inline options.
// Success/settlement callbacks must visibly invalidate, call a local callback that
// does, or delegate to an onX component callback parameter. The last shape records
// a parent handoff; review still checks what the parent does. Local bindings are
// resolved within the containing named function, not by a type/control-flow checker.
// Query keys, success vs settlement, execution, and semantic aliases remain review's
// job. Opaque options and unnamed sites fail closed rather than escaping inventory.
import ts from "typescript";
import { describe, expect, it } from "vitest";
import audit from "./path-mutation-audit.md?raw";

type Source = { path: string; text: string };
type Site = { key: string; location: string; declared: boolean };
type NamedFunction = ts.FunctionDeclaration | ts.FunctionExpression | ts.ArrowFunction;

function functionName(node: NamedFunction): string | undefined {
  if (!ts.isArrowFunction(node) && node.name) return node.name.text;
  return ts.isVariableDeclaration(node.parent) && ts.isIdentifier(node.parent.name)
    ? node.parent.name.text : undefined;
}

function ownerOf(node: ts.Node): NamedFunction | undefined {
  for (let parent = node.parent; parent; parent = parent.parent) {
    if ((ts.isFunctionDeclaration(parent) || ts.isFunctionExpression(parent) ||
      ts.isArrowFunction(parent)) && functionName(parent)) return parent;
  }
}

function property(object: ts.Node | undefined, name: string): ts.Expression | undefined {
  if (!object || !ts.isObjectLiteralExpression(object)) return;
  const member = object.properties.find((p) =>
    p.name && (ts.isIdentifier(p.name) || ts.isStringLiteral(p.name)) && p.name.text === name);
  if (member && ts.isPropertyAssignment(member)) return member.initializer;
  if (member && ts.isShorthandPropertyAssignment(member)) return member.name;
}

function callbackDeclaresInvalidation(callback: ts.Node, owner: NamedFunction): boolean {
  const locals = new Map<string, ts.Node>();
  const delegated = new Set<string>();
  for (const parameter of owner.parameters) {
    if (ts.isObjectBindingPattern(parameter.name)) {
      for (const binding of parameter.name.elements) {
        if (ts.isIdentifier(binding.name) && /^on[A-Z]/.test(binding.name.text)) {
          delegated.add(binding.name.text);
        }
      }
    }
  }
  function collect(node: ts.Node) {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) {
      locals.set(node.name.text, node.initializer);
    }
    if (ts.isFunctionDeclaration(node) && node.name) locals.set(node.name.text, node);
    ts.forEachChild(node, collect);
  }
  if (owner.body) collect(owner.body);

  const visiting = new Set<string>();
  function referenced(name: string): boolean {
    if (visiting.has(name)) return false;
    const local = locals.get(name);
    if (!local) return delegated.has(name);
    visiting.add(name);
    const result = inspect(local);
    visiting.delete(name);
    return result;
  }
  function inspect(node: ts.Node): boolean {
    if (ts.isIdentifier(node)) return referenced(node.text);
    if (ts.isCallExpression(node)) {
      const callee = node.expression;
      if (ts.isPropertyAccessExpression(callee) && callee.name.text === "invalidateQueries") {
        return true;
      }
      if (ts.isIdentifier(callee) && referenced(callee.text)) return true;
    }
    // Only calls or callback references count; a comment/string mentioning
    // invalidateQueries, or a bare onX identifier inside a body, is not evidence.
    let found = false;
    ts.forEachChild(node, (child) => {
      if (!ts.isIdentifier(child) && inspect(child)) found = true;
    });
    return found;
  }
  return inspect(callback);
}

function mutationSites(source: Source): Site[] {
  const file = ts.createSourceFile(source.path, source.text, ts.ScriptTarget.Latest, true);
  const hooks = new Set(["useMutation"]);
  for (const statement of file.statements) {
    if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier) ||
      statement.moduleSpecifier.text !== "@tanstack/react-query") continue;
    const bindings = statement.importClause?.namedBindings;
    if (bindings && ts.isNamedImports(bindings)) {
      for (const binding of bindings.elements) {
        if ((binding.propertyName ?? binding.name).text === "useMutation") hooks.add(binding.name.text);
      }
    }
  }
  const sites: Site[] = [];
  function visit(node: ts.Node) {
    if (ts.isCallExpression(node) &&
      (ts.isIdentifier(node.expression) && hooks.has(node.expression.text) ||
       ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === "useMutation")) {
      const owner = ownerOf(node);
      const component = owner && functionName(owner);
      const binding = ts.isVariableDeclaration(node.parent) && ts.isIdentifier(node.parent.name)
        ? node.parent.name.text : "<unnamed>";
      const stem = source.path.replace(/^src\//, "").replace(/\.tsx?$/, "");
      const label = stem === component ? stem : `${stem} / ${component ?? "<unnamed>"}`;
      const options = node.arguments[0];
      const reason = property(property(options, "meta"), "noInvalidationReason");
      const noInvalidation = reason && ts.isStringLiteral(reason) && reason.text.trim().length > 0;
      const callback = ["onSuccess", "onSettled"].some((name) => {
        const value = property(options, name);
        return value && owner && callbackDeclaresInvalidation(value, owner);
      });
      const line = file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1;
      sites.push({
        key: `${label}::${binding}`,
        location: `${source.path}:${line}`,
        declared: !!component && binding !== "<unnamed>" && !!options && ts.isObjectLiteralExpression(options) &&
          !options.properties.some(ts.isSpreadAssignment) && !!(callback || noInvalidation),
      });
    }
    ts.forEachChild(node, visit);
  }
  visit(file);
  return sites;
}

function inventoryViolations(sources: Source[], audit: string): string[] {
  const sites = sources.flatMap(mutationSites);
  const violations: string[] = [];
  if (!sites.length) violations.push("The mutation gate found no production useMutation call sites.");
  const section = audit.split("## Complete mutation inventory")[1]?.split(/\n## /)[0];
  if (!section) violations.push("Restore the Complete mutation inventory table in path-mutation-audit.md.");
  const rows = new Map<string, number>();
  for (const line of (section ?? "").split("\n")) {
    if (!line.startsWith("|")) continue;
    const cells = line.split("|").slice(1, -1).map((cell) => cell.trim());
    if (cells[0] === "File / component" || cells.every((cell) => /^[- :]+$/.test(cell))) continue;
    if (cells.length !== 3 || cells.some((cell) => !cell)) {
      violations.push(`Repair the inventory row (file/component, mutation, and reason required): ${line}`);
      continue;
    }
    const key = `${cells[0]}::${cells[1]}`;
    rows.set(key, (rows.get(key) ?? 0) + 1);
  }
  const live = new Set<string>();
  for (const site of sites) {
    if (live.has(site.key)) violations.push(`${site.location} ${site.key}: use a unique named mutation binding.`);
    live.add(site.key);
    if (!site.declared) violations.push(
      `${site.location} ${site.key}: record an onSuccess/onSettled invalidation callback or a non-empty literal meta.noInvalidationReason; use named bindings and inline options without spreads.`);
    const count = rows.get(site.key) ?? 0;
    if (count !== 1) violations.push(
      `${site.location} ${site.key}: add exactly one path-mutation-audit.md row explaining the decision (found ${count}).`);
  }
  for (const key of rows.keys()) {
    if (!live.has(key)) violations.push(`${key}: remove or rename the stale path-mutation-audit.md row; no live mutation matches.`);
  }
  return violations;
}

function productionSources(): Source[] {
  // Vite loads actual source text, without executing production modules or
  // requiring Node type declarations that this frontend does not depend on.
  const files = import.meta.glob<string>([
    "../**/*.{ts,tsx}", "!../test/**", "!../**/*.{test,spec,d}.{ts,tsx}",
  ], { eager: true, query: "?raw", import: "default" });
  return Object.entries(files)
    .map(([path, text]) => ({ path: path.replace(/^\.\.\//, "src/"), text }))
    .sort((a, b) => a.path.localeCompare(b.path));
}

const fixture = (options: string): Source[] => [{
  path: "src/Fixture.tsx",
  text: `function Widget({ onChanged }) {
    const refresh = () => qc.invalidateQueries({ queryKey: ["fixture"] });
    const save = useMutation(${options});
  }`,
}];
const table = (...rows: string[]) => `## Complete mutation inventory
| File / component | Mutation | Result and displayed state |
| --- | --- | --- |
${rows.map((row) => `| Fixture / Widget | ${row} | Recorded fixture decision. |`).join("\n")}`;

describe("frontend mutation invalidation boundary", () => {
  it("enforces declarations and the inventory against every production source file", () => {
    expect(inventoryViolations(productionSources(), audit)).toEqual([]);
  });

  it.each([
    '{ mutationFn: persist }',
    '{ mutationFn: persist, onSuccess: () => setDraft("") }',
    '{ mutationFn: persist, onSettled: undefined }',
    '{ mutationFn: persist, onSettled: unknownCallback }',
    '{ mutationFn: persist, meta: { noInvalidationReason: "  " } }',
    '{ mutationFn: persist, meta: { noInvalidationReason: dynamicReason } }',
    '{ mutationFn: persist, onSuccess: () => "invalidateQueries()" }',
    '{ mutationFn: persist, ...hiddenOptions }',
    'hiddenOptions',
    '',
  ])("rejects a site without a recorded decision: %s", (options) => {
    expect(inventoryViolations(fixture(options), table("save"))).toEqual([
      expect.stringContaining("src/Fixture.tsx:3 Fixture / Widget::save: record an onSuccess/onSettled"),
    ]);
  });

  it.each([
    '{ mutationFn: persist, onSuccess: () => qc.invalidateQueries({ queryKey: ["fixture"] }) }',
    '{ mutationFn: persist, onSettled: refresh }',
    '{ mutationFn: persist, onSuccess: () => { setDraft(""); refresh(); } }',
    '{ mutationFn: persist, onSettled: onChanged }',
    '{ mutationFn: persist, onSuccess: () => onChanged() }',
    '{ mutationFn: probe, meta: { noInvalidationReason: "Connection probe; no persisted query state." } }',
  ])("accepts an explicit decision: %s", (options) => {
    expect(inventoryViolations(fixture(options), table("save"))).toEqual([]);
  });

  it("rejects missing, duplicate, and stale inventory rows", () => {
    const sources = fixture('{ mutationFn: persist, onSettled: refresh }');
    expect(inventoryViolations(sources, table())).toEqual([
      expect.stringContaining("Fixture / Widget::save: add exactly one path-mutation-audit.md row"),
    ]);
    expect(inventoryViolations(sources, table("save", "save"))).toEqual([
      expect.stringContaining("Fixture / Widget::save: add exactly one path-mutation-audit.md row explaining the decision (found 2)"),
    ]);
    expect(inventoryViolations(sources, table("save", "deleted"))).toEqual([
      expect.stringContaining("Fixture / Widget::deleted: remove or rename the stale"),
    ]);
  });

  it("rejects duplicate live bindings and rows without an explanation", () => {
    const sources = fixture('{ mutationFn: persist, onSettled: refresh }');
    expect(inventoryViolations([...sources, ...sources], table("save"))).toContainEqual(
      expect.stringContaining("Fixture / Widget::save: use a unique named mutation binding"),
    );
    expect(inventoryViolations(sources, table("save").replace("Recorded fixture decision.", "")))
      .toContainEqual(expect.stringContaining("Repair the inventory row"));
  });

  it("parses aliases, namespace calls, and generics without counting imports, comments, or strings", () => {
    const text = `import { useMutation as mutate } from "@tanstack/react-query";
      import * as query from "@tanstack/react-query";
      // useMutation({ ignored: true })
      const example = "useMutation({ ignored: true })";
      function Widget() {
        const save = mutate<Result, Error>({ onSettled: () => qc.invalidateQueries({}) });
        const probe = query.useMutation({ meta: { noInvalidationReason: "Probe only." } });
      }`;
    const sources = [{ path: "src/Fixture.tsx", text }];
    expect(mutationSites(sources[0]).map((site) => site.key)).toEqual([
      "Fixture / Widget::save", "Fixture / Widget::probe",
    ]);
    expect(inventoryViolations(sources, table("save", "probe"))).toEqual([]);
  });
});
