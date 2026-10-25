// A disabled observer retains its error. Accept structural truth guards and
// preceding early returns; never assume isError/isFetching clears that error.
import ts from "typescript";
import { describe, expect, it } from "vitest";
import audit from "./ipc-boundary-audit.md?raw";
import { binding, checkInventory, hooks, inlineOptions, isFn, ownerOf, productionSources,
  property, resolve, walk, type Hook, type Source } from "./ipc-boundary-source";

function shape(node: ts.Expression): string {
  node = resolve(node);
  // Compare syntax trees, retaining literal text and ignoring only trivia.
  function tree(current: ts.Node): string {
    if (ts.isIdentifier(current) || ts.isLiteralExpression(current)) return `${current.kind}:${JSON.stringify(current.text)}`;
    const children: string[] = [];
    ts.forEachChild(current, (child) => { children.push(tree(child)); });
    return `${current.kind}(${children.join(",")})`;
  }
  return tree(node);
}
function terms(node: ts.Expression, truth = true): string[] {
  node = resolve(node);
  if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.ExclamationToken)
    return terms(node.operand, !truth);
  if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "Boolean" && node.arguments.length === 1)
    return terms(node.arguments[0], truth);
  if (!truth && ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.BarBarToken)
    return [...terms(node.left, false), ...terms(node.right, false)];
  if (truth && ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken)
    return [...terms(node.left), ...terms(node.right)];
  return [`${truth ? "+" : "-"}${shape(node)}`];
}
function returns(node: ts.Node): boolean {
  return ts.isReturnStatement(node) || ts.isBlock(node) && !!node.statements.length &&
    returns(node.statements[node.statements.length - 1]);
}
function guarded(error: ts.Node, enabled: ts.Expression, owner: NonNullable<Hook["owner"]>): boolean {
  const proven = new Set<string>();
  const add = (condition: ts.Expression, truth = true) => terms(condition, truth).forEach((t) => proven.add(t));
  for (let child = error, p = child.parent; p && p !== owner; child = p, p = p.parent) {
    if (ts.isBinaryExpression(p) && p.right === child && p.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken) add(p.left);
    if (ts.isConditionalExpression(p)) {
      if (p.whenTrue === child) add(p.condition);
      if (p.whenFalse === child) add(p.condition, false);
    }
    if (ts.isIfStatement(p)) {
      if (p.thenStatement === child) add(p.expression);
      if (p.elseStatement === child) add(p.expression, false);
    }
    if (ts.isBlock(p)) {
      for (const previous of p.statements) {
        if (previous === child) break;
        if (ts.isIfStatement(previous) && !previous.elseStatement && returns(previous.thenStatement)) add(previous.expression, false);
      }
    }
  }
  return terms(enabled).every((t) => proven.has(t));
}
function conditionalSites(sources: Source[]): Hook[] {
  return sources.flatMap((source) => hooks(source, "useQuery")).filter((site) => {
    if (!inlineOptions(site.options)) return true; // opaque options may hide enabled
    const enabled = property(site.options, "enabled");
    return enabled && ![ts.SyntaxKind.TrueKeyword, ts.SyntaxKind.FalseKeyword].includes(resolve(enabled).kind);
  });
}
const failureProperties = new Set(["error", "isError", "isLoadingError", "isRefetchError",
  "failureReason", "failureCount", "status"]);
function queryViolations(sources: Source[]): string[] {
  const errors: string[] = [];
  for (const site of conditionalSites(sources)) {
    const label = `${site.location} ${site.key}`;
    const enabled = property(site.options, "enabled");
    // A per-site review boundary for correct shapes this syntax-only gate cannot prove.
    // checkInventory requires this exact literal reason in the audit as well.
    const allowance = property(property(site.options, "meta"), "conditionalErrorBoundaryReason");
    if (allowance && ts.isStringLiteral(allowance) && allowance.text.trim()) continue;
    if (!ts.isVariableDeclaration(site.call.parent) || !ts.isIdentifier(site.call.parent.name) || !site.owner || site.key.includes("<unnamed>") || !inlineOptions(site.options) || !enabled || isFn(resolve(enabled))) {
      errors.push(`${label}: opaque conditional query; require named binding, inline options and a boolean condition.`);
      continue;
    }
    const declaration = site.call.parent;
    const owner = site.owner;
    walk(owner.body!, (node) => {
      if (!ts.isPropertyAccessExpression(node) || !failureProperties.has(node.name.text)) return;
      const subject = node.expression;
      if (!ts.isIdentifier(subject) || binding(subject.text, subject) !== declaration) return;
      let jsx = false;
      for (let p = node.parent; p && p !== owner; p = p.parent) {
        if (ts.isJsxExpression(p)) jsx = true;
      }
      // Error aliases and handoffs cannot silently evade the rendered-error gate.
      if (!jsx) errors.push(`${label}: opaque error use outside JSX; render the query error directly under its enabled guard.`);
      else if (ownerOf(node) !== owner || !guarded(node, enabled, owner))
        errors.push(`${label}: query error render must share its enabled condition.`);
    });
    // Destructuring/property forwarding is not covered by direct .error reads.
    walk(owner.body!, (node) => {
      if (!ts.isIdentifier(node) || binding(node.text, node) !== declaration || node === (declaration as ts.VariableDeclaration).name) return;
      const p = node.parent;
      if (ts.isPropertyAccessExpression(p) && p.expression === node) return;
      errors.push(`${label}: opaque query result use; use direct observer properties.`);
    });
  }
  return errors;
}
const source = (body: string): Source[] => [{ path: "src/Fixture.tsx", text: `function Widget({ open, mod }) { ${body} }` }];
const query = 'const hasUpstream = open && mod?.source === "gamebanana"; const updates = useQuery({ enabled: hasUpstream });';

describe("conditional query error boundary", () => {
  it("gates conditional query errors in production source", () => {
    expect(queryViolations(productionSources())).toEqual([]);
    expect(checkInventory("1", conditionalSites(productionSources()), audit)).toEqual([]);
  });
  it("rejects the ungated upstream error from #432", () => {
    expect(queryViolations(source('const updates = useQuery({ enabled: open && mod?.source === "gamebanana" }); return <CommandErrorNotice error={updates.error} />;')))
      .toEqual([expect.stringContaining("query error render must share its enabled condition")]);
  });
  it.each([...failureProperties])("guards retained failure property %s without an error read", (name) => {
    expect(queryViolations(source(`${query} return <>{updates.${name} === "error" && <p role="alert">Failed.</p>}</>;`)))
      .toContainEqual(expect.stringContaining("must share its enabled condition"));
    expect(queryViolations(source(`${query} return <>{hasUpstream && <p>{updates.${name}}</p>}</>;`))).toEqual([]);
  });
  it("allows stale data reads without treating them as failure messaging", () => {
    expect(queryViolations(source(`${query} return <p>{updates.data}</p>;`))).toEqual([]);
  });
  it("accepts a negated disjunction early return with Boolean coercion", () => {
    expect(queryViolations(source('const q = useQuery({ enabled: open && Boolean(mod) }); if (!open || !mod) return null; return <p>{q.error}</p>;'))).toEqual([]);
  });
  it.each([
    'const { data, error } = useQuery(OPTIONS); if (!open) return null; return <p>{error}</p>;',
    'const q = useQuery(OPTIONS); return q;',
    'const q = useQuery(OPTIONS); useEffect(() => { if (open && q.error) report(q.error); }); return <>{open && <p>{q.error}</p>}</>;',
  ])("accepts an explicitly reviewed correct shape: %s", (body) => {
    const sources = source(body.replace("OPTIONS", '{ enabled: open, meta: { conditionalErrorBoundaryReason: "Reviewed guarded consumer" } }'));
    expect(queryViolations(sources)).toEqual([]);
    const sites = conditionalSites(sources);
    const row = `| 1 | ${sites[0].key} | Reviewed guarded consumer | Consumer guard reviewed. |`;
    expect(checkInventory("1", sites, row)).toEqual([]);
    expect(checkInventory("1", sites, "  " + row)).toEqual([]);
    expect(checkInventory("1", sites, row.replace("Reviewed guarded consumer", "bogusHelper")))
      .toContainEqual(expect.stringContaining("declare exact conditional error allowance"));
  });
  it("accepts the fixed upstream guard from #432", () => {
    expect(queryViolations(source(`${query} return <>{hasUpstream && <CommandErrorNotice error={updates.error} />}</>;`))).toEqual([]);
  });
  it.each([
    'return <>{open && <CommandErrorNotice error={updates.error} />}</>;',
    'return <>{hasUpstream || <CommandErrorNotice error={updates.error} />}</>;',
    'return <>{!hasUpstream && <CommandErrorNotice error={updates.error} />}</>;',
    'return <>{hasUpstream ? null : <CommandErrorNotice error={updates.error} />}</>;',
    'if (!open) return null; return <CommandErrorNotice error={updates.error} />;',
    'return <>{updates.isError && <p>{updates.error.message}</p>}</>;',
  ])("rejects a missing, partial or reversed guard: %s", (render) => {
    expect(queryViolations(source(query + render))).toContainEqual(expect.stringContaining("must share its enabled condition"));
  });
  it.each([
    'return <>{hasUpstream ? <CommandErrorNotice error={updates.error} /> : null}</>;',
    'if (!hasUpstream) return null; return <CommandErrorNotice error={updates.error} />;',
    'return <>{open && mod?.source === "gamebanana" && <p>{updates.error.message}</p>}</>;',
    'if (hasUpstream) { return <p>{updates.error.message}</p>; } return null;',
  ])("accepts equivalent structural guards: %s", (render) => {
    expect(queryViolations(source(query + render))).toEqual([]);
  });
  it.each([
    'const updates = useQuery(options);',
    'const updates = useQuery({ ...options, enabled: open });',
    'const { error } = useQuery({ enabled: open });',
    'const updates = useQuery({ enabled: () => open });',
    `${query} const { error } = updates; return <p>{error}</p>;`,
    `${query} const error = updates.error; return <p>{error}</p>;`,
    `${query} return <Notice result={updates} />;`,
  ])("fails closed on opaque query/error shapes: %s", (body) => {
    expect(queryViolations(source(body)).length).toBeGreaterThan(0);
  });
  it("recognizes imported aliases, namespace calls, generics and parentheses", () => {
    const sources = [{ path: "src/Fixture.tsx", text: `import { useQuery as read } from "@tanstack/react-query";
      function Widget({ open }) { const a = read<Result>({ enabled: open });
        const b = rq.useQuery({ enabled: open });
        if (!open) return null; return <><p>{a.error}</p><p>{b.error}</p></>; }` }];
    expect(conditionalSites(sources)).toHaveLength(2);
    expect(queryViolations(sources)).toEqual([]);
  });
  it("does not borrow guards from a sibling component or nested callback", () => {
    expect(queryViolations(source(`${query} const f = () => <p>{updates.error}</p>; return hasUpstream && f();`)))
      .toContainEqual(expect.stringContaining("must share its enabled condition"));
  });
  it("leaves unconditional and manually triggered errors outside this rule", () => {
    expect(queryViolations(source('const a = useQuery({}); const b = useQuery({ enabled: false }); return <><p>{a.error}</p><p>{b.error}</p></>;'))).toEqual([]);
  });
  it("requires one explained inventory row per conditional query", () => {
    const sites = conditionalSites(source(query));
    expect(checkInventory("1", sites, "")).toContainEqual(expect.stringContaining("exactly one explained"));
    const row = '| 1 | Fixture / Widget::updates | none | Guarded error. |';
    expect(checkInventory("1", sites, row)).toEqual([]);
    expect(checkInventory("1", sites, "  " + row)).toEqual([]);
    expect(checkInventory("1", sites, row.replace("none", "bogusHelper")))
      .toContainEqual(expect.stringContaining('declare exact conditional error allowance "none"'));
    expect(checkInventory("1", sites, row + '\n' + row)).toContainEqual(expect.stringContaining("exactly one explained"));
    expect(checkInventory("1", [], row)).toContainEqual(expect.stringContaining("stale"));
  });
});
