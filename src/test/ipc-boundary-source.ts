// Shared only by the two IPC gates. The older invalidation gate stays independent.
import ts from "typescript";

export type Source = { path: string; text: string };
export type Fn = ts.FunctionDeclaration | ts.FunctionExpression | ts.ArrowFunction;
export const isFn = (node: ts.Node): node is Fn => ts.isFunctionDeclaration(node) ||
  ts.isFunctionExpression(node) || ts.isArrowFunction(node);
export function walk(node: ts.Node, visit: (node: ts.Node) => void) {
  visit(node);
  ts.forEachChild(node, (child) => walk(child, visit));
}
export function nameOf(node: Fn): string | undefined {
  if (!ts.isArrowFunction(node) && node.name) return node.name.text;
  return ts.isVariableDeclaration(node.parent) && ts.isIdentifier(node.parent.name)
    ? node.parent.name.text : undefined;
}
export function ownerOf(node: ts.Node): Fn | undefined {
  for (let p = node.parent; p; p = p.parent) if (isFn(p) && nameOf(p)) return p;
}
export function unwrap(node: ts.Expression): ts.Expression {
  while (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) ||
    ts.isSatisfiesExpression(node) || ts.isNonNullExpression(node)) node = node.expression;
  return node;
}
export function property(node: ts.Node | undefined, name: string): ts.Expression | undefined {
  if (!node || !ts.isObjectLiteralExpression(node)) return;
  const member = node.properties.find((p) => p.name &&
    (ts.isIdentifier(p.name) || ts.isStringLiteral(p.name)) && p.name.text === name);
  if (member && ts.isPropertyAssignment(member)) return member.initializer;
  if (member && ts.isShorthandPropertyAssignment(member)) return member.name;
}
export function inlineOptions(node: ts.Node | undefined): node is ts.ObjectLiteralExpression {
  return !!node && ts.isObjectLiteralExpression(node) && node.properties.every((p) =>
    (ts.isPropertyAssignment(p) || ts.isShorthandPropertyAssignment(p)) &&
    (ts.isIdentifier(p.name) || ts.isStringLiteral(p.name))) &&
    new Set(node.properties.map((p) => p.name!.getText())).size === node.properties.length;
}
// Resolve the nearest lexical declaration. Do not borrow a same-named binding
// from a sibling/nested component. Parameters shadow outer constants as well.
export function binding(name: string, at: ts.Node): ts.Node | undefined {
  for (let scope = at.parent; scope; scope = scope.parent) {
    if (isFn(scope)) {
      for (const param of scope.parameters) {
        let found = false;
        walk(param.name, (n) => { if (ts.isIdentifier(n) && n.text === name) found = true; });
        if (found) return param;
      }
    }
    if (!ts.isBlock(scope) && !ts.isSourceFile(scope)) continue;
    for (const statement of scope.statements) {
      if (ts.isFunctionDeclaration(statement) && statement.name?.text === name) return statement;
      if (!ts.isVariableStatement(statement)) continue;
      for (const decl of statement.declarationList.declarations) {
        let found = false;
        walk(decl.name, (n) => { if (ts.isIdentifier(n) && n.text === name) found = true; });
        if (found) return decl;
      }
    }
  }
}
export function resolve(node: ts.Expression, seen = new Set<ts.Node>()): ts.Expression {
  node = unwrap(node);
  if (!ts.isIdentifier(node)) return node;
  const decl = binding(node.text, node);
  if (!decl || !ts.isVariableDeclaration(decl) || !decl.initializer ||
    !ts.isIdentifier(decl.name) || seen.has(decl) ||
    !(decl.parent.flags & ts.NodeFlags.Const)) return node;
  seen.add(decl);
  return resolve(decl.initializer, seen);
}
export type Hook = { call: ts.CallExpression; owner?: Fn; key: string; location: string; options?: ts.Expression };
export function hooks(source: Source, hook: "useQuery" | "useMutation"): Hook[] {
  const file = ts.createSourceFile(source.path, source.text, ts.ScriptTarget.Latest, true);
  const names = new Set([hook]);
  for (const stmt of file.statements) {
    if (!ts.isImportDeclaration(stmt) || !ts.isStringLiteral(stmt.moduleSpecifier) ||
      stmt.moduleSpecifier.text !== "@tanstack/react-query") continue;
    const imports = stmt.importClause?.namedBindings;
    if (imports && ts.isNamedImports(imports)) for (const spec of imports.elements) {
      if ((spec.propertyName ?? spec.name).text === hook) names.add(spec.name.text as typeof hook);
    }
  }
  const result: Hook[] = [];
  walk(file, (node) => {
    if (!ts.isCallExpression(node) || !(ts.isIdentifier(node.expression) && names.has(node.expression.text as typeof hook) ||
      ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === hook)) return;
    const owner = ownerOf(node);
    const stem = source.path.replace(/^src\//, "").replace(/\.tsx?$/, "");
    const label = stem === (owner && nameOf(owner)) ? stem : `${stem} / ${owner && nameOf(owner) || "<unnamed>"}`;
    const declaration = node.parent;
    const variable = ts.isVariableDeclaration(declaration) && ts.isIdentifier(declaration.name)
      ? declaration.name.text : "<unnamed>";
    result.push({ call: node, owner, key: `${label}::${variable}`,
      location: `${source.path}:${file.getLineAndCharacterOfPosition(node.getStart()).line + 1}`,
      options: node.arguments[0] });
  });
  return result;
}
export function productionSources(): Source[] {
  const files = import.meta.glob<string>([
    "../**/*.{ts,tsx}", "!../test/**", "!../**/*.{test,spec,d}.{ts,tsx}",
  ], { eager: true, query: "?raw", import: "default" });
  return Object.entries(files).map(([path, text]) => ({ path: path.replace(/^\.\.\//, "src/"), text }))
    .sort((a, b) => a.path.localeCompare(b.path));
}
export type Row = { rule: string; key: string; opaque: string; reason: string };
export function inventory(audit: string): Row[] {
  return audit.split("\n").filter((line) => /^\| [12] \|/.test(line)).map((line) => {
    const cells = line.split("|").slice(1, -1).map((cell) => cell.trim());
    return { rule: cells[0], key: cells[1], opaque: cells[2], reason: cells[3] };
  });
}
export function checkInventory(rule: string, sites: Hook[], audit: string): string[] {
  const rows = inventory(audit).filter((row) => row.rule === rule);
  const errors: string[] = [];
  if (!sites.length) errors.push(`Rule ${rule}: no inventoried sites found.`);
  for (const site of sites) {
    if (site.key.includes("<unnamed>") || sites.filter((s) => s.key === site.key).length !== 1)
      errors.push(`${site.key}: require unique named owner and binding.`);
    const matches = rows.filter((row) => row.key === site.key);
    if (matches.length !== 1 || !matches[0].reason || !matches[0].opaque)
      errors.push(`${site.key}: require exactly one explained rule ${rule} inventory row.`);
  }
  for (const row of rows) if (!sites.some((s) => s.key === row.key)) errors.push(`${row.key}: stale rule ${rule} inventory row.`);
  return errors;
}
