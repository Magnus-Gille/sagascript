#!/usr/bin/env node
/**
 * Parser-backed inventory and archived-measurement assembly for the #331 code-health trial.
 *
 * This is an exploration aid, not a CI gate. It deliberately reports static
 * heuristics separately from compiler/analyzer measurements. Generated and
 * private inputs are excluded.
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { parse as parseSvelte } from "svelte/compiler";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const base = "24cbf972ddd92ca488331d0d65ee1e313dd87d95";
const output = join(root, "docs/research/code-health-2026-10-09/evidence.json");
const tracked = new Set(execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" }).split("\0"));
const measurements = JSON.parse(readFileSync(join(dirname(output), "measured-baseline.json"), "utf8"));
// Never relabel later application code with a frozen trial result.
execFileSync("git", ["diff", "--quiet", base, "--", "src", "src-tauri"], { cwd: root });

function git(...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

function walk(dir, predicate, result = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if ([".git", "node_modules", "target", ".build", "build"].includes(entry.name)) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, predicate, result);
    else if (tracked.has(relative(root, path)) && predicate(path)) result.push(path);
  }
  return result.sort();
}

function lines(path) {
  return readFileSync(path, "utf8").split("\n").length - 1;
}

function walkAst(node, counts, seen = new WeakSet()) {
  if (!node || typeof node !== "object") return;
  if (seen.has(node)) return;
  seen.add(node);
  if (typeof node.type === "string") {
    if (["FunctionDeclaration", "FunctionExpression", "ArrowFunctionExpression"].includes(node.type)) counts.functions += 1;
    if (["IfStatement", "ForStatement", "ForOfStatement", "ForInStatement", "WhileStatement", "DoWhileStatement", "SwitchCase", "ConditionalExpression", "CatchClause"].includes(node.type)) counts.branches += 1;
    if (node.type === "LogicalExpression" && ["&&", "||", "??"].includes(node.operator)) counts.branches += 1;
    if (["IfBlock", "EachBlock", "AwaitBlock", "KeyBlock"].includes(node.type)) counts.template_blocks += 1;
  }
  for (const [key, value] of Object.entries(node)) {
    if (key === "loc" || key === "start" || key === "end") continue;
    if (Array.isArray(value)) value.forEach((child) => walkAst(child, counts, seen));
    else if (value && typeof value === "object") walkAst(value, counts, seen);
  }
}

function astMetrics(path, language) {
  const counts = { lines: lines(path), functions: 0, branches: 0, template_blocks: 0 };
  const source = readFileSync(path, "utf8");
  if (language === "svelte") {
    const ast = parseSvelte(source);
    walkAst(ast.instance?.content, counts);
    walkAst(ast.html, counts);
  } else if (language === "typescript") {
    const ast = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    if (ast.parseDiagnostics.length) throw new Error(`TypeScript parse failed: ${path}`);
    const visit = (node) => {
      if (ts.isFunctionDeclaration(node) || ts.isFunctionExpression(node) || ts.isArrowFunction(node)) counts.functions += 1;
      if (ts.isIfStatement(node) || ts.isForStatement(node) || ts.isForOfStatement(node) || ts.isForInStatement(node) || ts.isWhileStatement(node) || ts.isDoStatement(node) || ts.isCaseClause(node) || ts.isConditionalExpression(node) || ts.isCatchClause(node)) counts.branches += 1;
      if (ts.isBinaryExpression(node) && [ts.SyntaxKind.AmpersandAmpersandToken, ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken].includes(node.operatorToken.kind)) counts.branches += 1;
      ts.forEachChild(node, visit);
    };
    visit(ast);
  } else {
    counts.parser = "not_run";
  }
  return counts;
}

function grouped(files, language) {
  return files.map((path) => ({ path: relative(root, path), language, ...astMetrics(path, language) }))
    .sort((a, b) => (b.branches + b.template_blocks) - (a.branches + a.template_blocks) || a.path.localeCompare(b.path));
}

const rustFiles = walk(join(root, "src-tauri"), (p) => p.endsWith(".rs"));
const frontendFiles = walk(join(root, "src"), (p) => p.endsWith(".svelte") || p.endsWith(".ts"));
const swiftFiles = walk(join(root, "src-tauri/engine-host/coreml"), (p) => p.endsWith(".swift") && !p.endsWith("BuildInfo.swift"));

let churn = { window_start: "2026-09-09", window_end: "2026-10-10", paths: {} };
try {
  const rows = git("log", "--since=2026-09-09T00:00:00Z", "--until=2026-10-10T00:00:00Z", "--format=", "--numstat", base, "--", "src-tauri", "src").split("\n").filter(Boolean);
  for (const row of rows) {
    const [add, del, path] = row.split("\t");
    if (/^\d+$/.test(add) && /^\d+$/.test(del)) {
      churn.paths[path] ??= { commits: 0, additions: 0, deletions: 0 };
      churn.paths[path].commits += 1;
      churn.paths[path].additions += Number(add);
      churn.paths[path].deletions += Number(del);
    }
  }
} catch (error) {
  churn = { status: "unavailable", reason: error.message };
}

const evidence = {
  schema_version: 1,
  generated_at: "2026-10-09",
  repository: "Magnus-Gille/sagascript",
  base_commit: base,
  working_tree_head: git("rev-parse", "HEAD"),
  scope: {
    included: ["Rust/Tauri", "Svelte/TypeScript", "Swift CoreML engine host"],
    excluded: ["audio/private recordings", "runtime refactors", "CI gate adoption", "dead-code deletion", "signing/release/deploy"],
  },
  inventory: {
    rust: grouped(rustFiles, "rust"),
    frontend: {
      svelte: grouped(frontendFiles.filter((p) => p.endsWith(".svelte")), "svelte"),
      typescript: grouped(frontendFiles.filter((p) => p.endsWith(".ts")), "typescript"),
    },
    swift: grouped(swiftFiles, "swift"),
  },
  churn,
  ...measurements,
};

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`);
console.log(output);
