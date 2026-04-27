import { tokenize, type Token } from "./kql-lexer";
import {
  fieldsFor,
  tablesList,
  KQL_KEYWORDS,
  KQL_FUNCTIONS,
  KQL_OPERATORS,
} from "./kql-schema";

export interface Suggestion {
  label: string;
  kind: "table" | "keyword" | "field" | "operator" | "function" | "value";
  insert: string;
}

export function suggest(query: string, cursor: number): Suggestion[] {
  const before = query.slice(0, cursor);
  const tokens = tokenize(before);
  const meaningful = tokens.filter((t) => t.kind !== "whitespace");

  const table = detectTable(meaningful);
  const partial = extractPartial(before, tokens);
  const needle = partial.toLowerCase();
  const ctx = classifyContext(meaningful);

  let items: Suggestion[];
  switch (ctx) {
    case "table":
      items = tablesList().map((t) => ({ label: t, kind: "table", insert: t }));
      break;
    case "keyword":
      items = [
        ...KQL_KEYWORDS.map((k) => ({ label: k, kind: "keyword" as const, insert: k })),
        ...KQL_FUNCTIONS.map((f) => ({ label: `${f}()`, kind: "function" as const, insert: `${f}(` })),
      ];
      break;
    case "field":
      items = fieldsFor(table).map((f) => ({ label: f.name, kind: "field" as const, insert: f.name }));
      break;
    case "operator":
      items = KQL_OPERATORS.map((o) => ({ label: o, kind: "operator" as const, insert: o }));
      break;
    case "value": {
      const field = findPrecedingField(meaningful);
      const fieldDef = fieldsFor(table).find((f) => f.name === field);
      items = (fieldDef?.examples ?? []).map((ex) => ({
        label: ex,
        kind: "value" as const,
        insert: ex,
      }));
      break;
    }
    default:
      items = [];
  }

  if (!needle) return items.slice(0, 12);
  return items
    .filter((s) => s.label.toLowerCase().includes(needle))
    .slice(0, 12);
}

type Context = "table" | "keyword" | "field" | "operator" | "value" | "none";

function classifyContext(tokens: Token[]): Context {
  const last = tokens[tokens.length - 1];
  if (!last) return "table";

  if (last.kind === "pipe") return "keyword";

  if (last.kind === "keyword") {
    const kw = last.text.toLowerCase();
    if (kw === "where" || kw === "extend" || kw === "project") return "field";
    if (kw === "by") return "field";
    return "field";
  }

  if (last.kind === "field" || last.kind === "ident") {
    const prev = tokens.length >= 2 ? tokens[tokens.length - 2] : null;
    if (prev?.kind === "operator") return "none";
    if (!prev || prev.kind === "pipe") return "table";
    return "operator";
  }

  if (last.kind === "operator") return "value";
  if (last.kind === "table") return "keyword";
  if (last.kind === "string" || last.kind === "number" || last.kind === "function")
    return "keyword";

  return "none";
}

function detectTable(tokens: Token[]): string {
  const tableToken = tokens.find((t) => t.kind === "table");
  return tableToken?.text ?? "DeviceLog";
}

function findPrecedingField(tokens: Token[]): string | null {
  for (let i = tokens.length - 1; i >= 0; i--) {
    const t = tokens[i];
    if (t && t.kind === "field") return t.text;
  }
  return null;
}

function extractPartial(before: string, tokens: Token[]): string {
  if (tokens.length === 0) return before.trim();
  const last = tokens[tokens.length - 1];
  if (!last) return before.trim();
  if (last.kind === "whitespace") return "";
  if (last.kind === "pipe" || last.kind === "operator") return "";
  return last.text;
}
