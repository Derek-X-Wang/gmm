// A presence-keyed query reading states its last value as fact. This gate does
// not forbid that; it freezes the set. Every reading not routed through
// readEvidence carries one inventory row naming the exact observer properties
// it keys off and why asserting them there is honest. A new reading, or a new
// property on an inventoried one, fails until the row says so.
import ts from "typescript";
import { describe, expect, it } from "vitest";
import audit from "./ipc-boundary-audit.md?raw";
import { binding, checkInventory, hooks, inventory, isFn, productionSources,
  unwrap, walk, type Hook, type Source } from "./ipc-boundary-source";

// Observer properties that say something about the value or about how the last
// read went. Reading any of them off the observer is a claim this gate records.
const claimProperties = new Set(["data", "dataUpdatedAt", "error", "errorUpdateCount",
  "errorUpdatedAt", "failureCount", "failureReason", "fetchStatus", "isEnabled", "isError",
  "isFetched", "isFetchedAfterMount", "isFetching", "isInitialLoading", "isLoading",
  "isLoadingError", "isPaused", "isPending", "isPlaceholderData", "isRefetchError",
  "isRefetching", "isStale", "isSuccess", "promise", "status"]);
// Asking for another read claims nothing about the value already held.
const controlProperties = new Set(["refetch"]);
const forwarded = "<forwarded>";

function readsEvidence(node: ts.Node): boolean {
  let current: ts.Node = node;
  // Climb out of parentheses and casts so a wrapped argument still counts.
  while (current.parent && ts.isExpression(current.parent) &&
    unwrap(current.parent) === current) current = current.parent;
  const call = current.parent;
  if (!call || !ts.isCallExpression(call) || !call.arguments.includes(current as ts.Expression)) return false;
  const callee = unwrap(call.expression);
  return ts.isIdentifier(callee) && callee.text === "readEvidence";
}
// `useQueries` hands back an array of readings; mapping each one through
// readEvidence is the same consumption as calling it on a single observer.
function mapsEvidence(access: ts.PropertyAccessExpression): boolean {
  if (access.name.text !== "map" || !ts.isCallExpression(access.parent) ||
    access.parent.expression !== access) return false;
  const callback = unwrap(access.parent.arguments[0] ?? access.parent.expression);
  if (ts.isIdentifier(callback)) return callback.text === "readEvidence";
  if (!isFn(callback) || !callback.body) return false;
  const parameter = callback.parameters[0]?.name;
  if (!parameter || !ts.isIdentifier(parameter)) return false;
  let mapped = false;
  walk(callback.body, (node) => {
    if (ts.isIdentifier(node) && node.text === parameter.text && readsEvidence(node)) mapped = true;
  });
  return mapped;
}

/**
 * False where the identifier is a name being written rather than a value being
 * read: the `data` in `q.data`, the `mods` in `mods={…}`, a declared binding.
 * `binding()` happily resolves those to a same-named query observer, and
 * `conflicts.data.conflicts` would otherwise read as a use of `conflicts`.
 * An unlisted position counts as a reference, so the gate fails closed.
 */
function reference(node: ts.Identifier): boolean {
  const parent = node.parent;
  if (ts.isPropertyAccessExpression(parent)) return parent.expression === node;
  if (ts.isQualifiedName(parent)) return parent.left === node;
  if (ts.isBindingElement(parent) || ts.isImportSpecifier(parent) || ts.isExportSpecifier(parent)) return false;
  if (ts.isJsxAttribute(parent) || ts.isPropertyAssignment(parent)) return parent.name !== node;
  if (ts.isJsxOpeningElement(parent) || ts.isJsxSelfClosingElement(parent) || ts.isJsxClosingElement(parent))
    return parent.tagName !== node;
  if (ts.isVariableDeclaration(parent) || ts.isParameter(parent) || ts.isFunctionDeclaration(parent) ||
    ts.isFunctionExpression(parent) || ts.isClassDeclaration(parent) || ts.isMethodDeclaration(parent) ||
    ts.isPropertyDeclaration(parent) || ts.isPropertySignature(parent) || ts.isEnumMember(parent) ||
    ts.isTypeAliasDeclaration(parent) || ts.isInterfaceDeclaration(parent) ||
    ts.isTypeParameterDeclaration(parent)) return parent.name !== node;
  return true;
}

type Reading = Hook & { claims: Set<string>; errors: string[] };
/**
 * Every `useQuery`/`useQueries` call that produces a reading, with the exact
 * observer properties its owner keys off. An empty claim set means the reading
 * reaches the surface only through readEvidence, so it needs no row.
 */
function queryReadings(sources: Source[]): Reading[] {
  const sites = sources.flatMap((source) => [...hooks(source, "useQuery"), ...hooks(source, "useQueries")]);
  const readings: Reading[] = [];
  for (const site of sites) {
    // A call whose result is discarded makes no claim: it only keeps the query
    // observed. There is nothing to read, so there is nothing to inventory.
    if (ts.isExpressionStatement(site.call.parent)) continue;
    const reading: Reading = { ...site, claims: new Set(), errors: [] };
    readings.push(reading);
    const declaration = site.call.parent;
    if (!ts.isVariableDeclaration(declaration) || !site.owner || !site.owner.body) {
      reading.errors.push("opaque query reading; bind the result to a named constant inside a named owner");
      continue;
    }
    if (!ts.isIdentifier(declaration.name)) {
      // Destructuring reads the properties it names, and a rest element reads
      // whatever is left, which this gate cannot enumerate.
      if (!ts.isObjectBindingPattern(declaration.name)) { reading.claims.add(forwarded); continue; }
      for (const element of declaration.name.elements) {
        const name = (element.propertyName ?? element.name);
        if (element.dotDotDotToken || !ts.isIdentifier(name)) reading.claims.add(forwarded);
        else if (claimProperties.has(name.text)) reading.claims.add(name.text);
        else if (!controlProperties.has(name.text)) reading.claims.add(forwarded);
      }
      continue;
    }
    const owner = site.owner;
    walk(owner.body!, (node) => {
      if (!ts.isIdentifier(node) || !reference(node) ||
        binding(node.text, node) !== declaration) return;
      if (readsEvidence(node)) return;
      const access = node.parent;
      if (!ts.isPropertyAccessExpression(access) || access.expression !== node) {
        reading.claims.add(forwarded);
        return;
      }
      if (claimProperties.has(access.name.text)) reading.claims.add(access.name.text);
      else if (!controlProperties.has(access.name.text) && !mapsEvidence(access)) reading.claims.add(forwarded);
    });
  }
  return readings;
}
const presenceKeyed = (sources: Source[]): Reading[] =>
  queryReadings(sources).filter((reading) => reading.claims.size > 0);

function evidenceViolations(sources: Source[], audit: string): string[] {
  const readings = queryReadings(sources);
  const errors = readings.flatMap((r) => r.errors.map((error) => `${r.location} ${r.key}: ${error}.`));
  const sites = readings.filter((reading) => reading.claims.size > 0);
  errors.push(...checkInventory("3", sites, audit));
  for (const site of sites) {
    const expected = [...site.claims].sort().join(", ");
    const row = inventory(audit).find((r) => r.rule === "3" && r.key === site.key);
    if (row?.opaque !== expected)
      errors.push(`${site.key}: declare exact presence-keyed reads "${expected}" with a written reason.`);
  }
  return errors;
}

const source = (body: string, path = "src/Fixture.tsx"): Source[] =>
  [{ path, text: `function Widget() { ${body} }` }];

describe("query evidence ratchet", () => {
  it("inventories every presence-keyed query reading in production source", () => {
    expect(evidenceViolations(productionSources(), audit)).toEqual([]);
  });
  it("covers useQuery and useQueries readings across production source", () => {
    const readings = queryReadings(productionSources());
    expect(readings.length).toBeGreaterThan(40);
    expect(new Set(readings.map((r) => r.location.split(":")[0])).size).toBeGreaterThan(10);
    expect(readings.some((r) => r.key === "App / MainApp::installPaths")).toBe(true);
  });
  it("asks nothing of a reading consumed only through readEvidence", () => {
    expect(presenceKeyed(source('const q = useQuery({}); const e = readEvidence(q); return <p>{e.kind}</p>;'))).toEqual([]);
    expect(presenceKeyed(source('const q = useQuery({}); return <p>{readEvidence(q).kind}</p>;'))).toEqual([]);
    expect(presenceKeyed(source('const q = useQuery({}); const e = readEvidence((q)); return <p>{e.kind}</p>;'))).toEqual([]);
  });
  it("treats a refetch control as no claim about the value", () => {
    expect(presenceKeyed(source('const q = useQuery({}); return <button onClick={() => void q.refetch()}>Retry</button>;'))).toEqual([]);
  });
  it("accepts a useQueries collection mapped through readEvidence", () => {
    for (const map of ["readEvidence", "(one) => readEvidence(one)", "(one) => { return readEvidence(one); }"]) {
      expect(presenceKeyed(source(`const qs = useQueries({ queries: [] }); const e = qs.map(${map}); return <p>{e.length}</p>;`))).toEqual([]);
    }
  });
  it.each([...claimProperties])("records the presence-keyed read of %s", (name) => {
    const sites = presenceKeyed(source(`const q = useQuery({}); return <p>{q.${name}}</p>;`));
    expect(sites.map((s) => [...s.claims])).toEqual([[name]]);
  });
  it("does not mistake a property or JSX prop named after the reading for a use of it", () => {
    const sites = presenceKeyed(source('const conflicts = useQuery({}); return <List conflicts={conflicts.data.conflicts} />;'));
    expect(sites.map((s) => [...s.claims])).toEqual([["data"]]);
  });
  it("records every property one reading keys off, not just the first", () => {
    const sites = presenceKeyed(source('const q = useQuery({}); return <p>{q.isLoading ? "…" : q.data}</p>;'));
    expect([...sites[0].claims].sort()).toEqual(["data", "isLoading"]);
  });
  it("records destructured properties and fails closed on a rest element", () => {
    expect([...presenceKeyed(source('const { data, isSuccess, refetch } = useQuery({});'))[0].claims].sort())
      .toEqual(["data", "isSuccess"]);
    expect([...presenceKeyed(source('const { data, ...rest } = useQuery({});'))[0].claims].sort())
      .toEqual(["<forwarded>", "data"]);
  });
  it.each([
    'const q = useQuery({}); return <Notice result={q} />;',
    'const q = useQuery({}); const alias = q; return <p>{alias.data}</p>;',
    'const qs = useQueries({ queries: [] }); const one = qs[0]; return <p>{one.data}</p>;',
    'const qs = useQueries({ queries: [] }); return <p>{qs.map((one) => one.data)}</p>;',
    'const q = useQuery({}); report(q); return null;',
  ])("fails closed on a reading it cannot follow: %s", (body) => {
    expect(presenceKeyed(source(body)).map((s) => [...s.claims])).toContainEqual(["<forwarded>"]);
  });
  it("leaves an unread observer subscription outside the rule", () => {
    expect(queryReadings(source('useQuery({ queryKey: ["session"] }); return null;'))).toEqual([]);
  });
  it("fails closed on a reading with no named binding", () => {
    expect(evidenceViolations(source('return <p>{useQuery({}).data}</p>;'), ""))
      .toContainEqual(expect.stringContaining("bind the result to a named constant"));
  });
  it("recognizes imported aliases, namespace calls and generics", () => {
    const sources = [{ path: "src/Fixture.tsx", text: `import { useQuery as read, useQueries as readAll } from "@tanstack/react-query";
      function Widget() { const a = read<Result>({}); const b = rq.useQuery({}); const c = readAll({ queries: [] });
        const d = rq.useQueries({ queries: [] });
        return <p>{a.data}{b.data}{c.length}{d.length}</p>; }` }];
    expect(presenceKeyed(sources).map((s) => s.key))
      .toEqual(["Fixture / Widget::a", "Fixture / Widget::b", "Fixture / Widget::c", "Fixture / Widget::d"]);
  });
  it("does not borrow a same-named reading from a sibling component", () => {
    const sources = [{ path: "src/Fixture.tsx", text: `function Other() { const q = useQuery({}); return <p>{q.data}</p>; }
      function Widget() { const q = useQuery({}); const e = readEvidence(q); return <p>{e.kind}</p>; }` }];
    expect(presenceKeyed(sources).map((s) => s.key)).toEqual(["Fixture / Other::q"]);
  });
  it("requires one explained row per presence-keyed reading, and rejects a stale one", () => {
    const sites = presenceKeyed(source('const q = useQuery({}); return <p>{q.data}</p>;'));
    const row = '| 3 | Fixture / Widget::q | data | A stale list is visibly stale beside it. |';
    expect(evidenceViolations(source('const q = useQuery({}); return <p>{q.data}</p>;'), ""))
      .toContainEqual(expect.stringContaining("require exactly one explained rule 3 inventory row"));
    expect(checkInventory("3", sites, row)).toEqual([]);
    expect(checkInventory("3", sites, "  " + row)).toEqual([]);
    expect(checkInventory("3", sites, row + "\n" + row))
      .toContainEqual(expect.stringContaining("require exactly one explained rule 3 inventory row"));
    expect(checkInventory("3", [], row)).toContainEqual(expect.stringContaining("stale rule 3 inventory row"));
  });
  it("rejects an unexplained row and a row that understates what the reading keys off", () => {
    const body = 'const q = useQuery({}); return <p>{q.isSuccess ? q.data : null}</p>;';
    expect(evidenceViolations(source(body), '| 3 | Fixture / Widget::q | data, isSuccess |  |'))
      .toContainEqual(expect.stringContaining("require exactly one explained rule 3 inventory row"));
    expect(evidenceViolations(source(body), '| 3 | Fixture / Widget::q | data | Reviewed. |'))
      .toContainEqual(expect.stringContaining('declare exact presence-keyed reads "data, isSuccess"'));
    expect(evidenceViolations(source(body), '| 3 | Fixture / Widget::q | data, isSuccess | Reviewed. |')).toEqual([]);
  });
  it("goes green for a reading migrated to readEvidence once its row is removed", () => {
    const before = source('const q = useQuery({}); return <p>{q.data}</p>;');
    const after = source('const q = useQuery({}); return <p>{readEvidence(q).value}</p>;');
    const row = '| 3 | Fixture / Widget::q | data | Reviewed. |';
    expect(evidenceViolations(before, row)).toEqual([]);
    expect(evidenceViolations(after, row)).toContainEqual(expect.stringContaining("stale rule 3 inventory row"));
    expect(presenceKeyed(after)).toEqual([]);
  });
  it("does not read claims out of comments or strings", () => {
    expect(queryReadings(source(`// const ignored = useQuery({});
      const text = 'useQuery({})'; return null;`))).toEqual([]);
  });
});
