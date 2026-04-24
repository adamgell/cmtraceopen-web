// Tiny lexer for the KQL subset shown by the query bar. Not a parser — it
// emits a flat token stream that the syntax-highlight renderer can colourise.
// Unknown identifiers fall through to `ident` so the UI can still show them
// (the stubbed executor doesn't enforce schema correctness yet).

import { fieldsFor, tablesList, KQL_KEYWORDS, KQL_FUNCTIONS, KQL_OPERATORS } from "./kql-schema";

export type TokenKind =
  | "table"
  | "pipe"
  | "keyword"
  | "function"
  | "operator"
  | "field"
  | "ident"
  | "string"
  | "number"
  | "whitespace";

export interface Token {
  kind: TokenKind;
  text: string;
  start: number;
  end: number;
}

const FIELD_NAMES = new Set<string>(
  tablesList().flatMap((t) => fieldsFor(t).map((f) => f.name))
);
const TABLES = new Set<string>(tablesList());
const KEYWORDS = new Set<string>(KQL_KEYWORDS);
const FUNCTIONS = new Set<string>(KQL_FUNCTIONS);
const OPERATORS = [...KQL_OPERATORS].sort((a, b) => b.length - a.length);

function isIdentStart(c: string | undefined): boolean {
  return c !== undefined && /[A-Za-z_]/.test(c);
}
function isIdentPart(c: string | undefined): boolean {
  return c !== undefined && /[A-Za-z0-9_]/.test(c);
}

function readWhile(input: string, i: number, pred: (c: string | undefined) => boolean): number {
  while (i < input.length && pred(input[i])) i++;
  return i;
}

export function tokenize(input: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < input.length) {
    const c = input[i];
    const start = i;

    if (c !== undefined && /\s/.test(c)) {
      i = readWhile(input, i, (ch) => ch !== undefined && /\s/.test(ch));
      tokens.push({ kind: "whitespace", text: input.slice(start, i), start, end: i });
      continue;
    }

    if (c === "|") {
      i++;
      tokens.push({ kind: "pipe", text: "|", start, end: i });
      continue;
    }

    if (c === '"') {
      i++;
      while (i < input.length && input[i] !== '"') i++;
      if (i < input.length) i++; // consume closing quote
      tokens.push({ kind: "string", text: input.slice(start, i), start, end: i });
      continue;
    }

    if (c !== undefined && /[0-9]/.test(c)) {
      i = readWhile(input, i, (ch) => ch !== undefined && /[0-9a-zA-Z]/.test(ch));
      tokens.push({ kind: "number", text: input.slice(start, i), start, end: i });
      continue;
    }

    // Operator match (longest first).
    let matchedOp = "";
    for (const op of OPERATORS) {
      if (input.startsWith(op, i)) {
        matchedOp = op;
        break;
      }
    }
    if (matchedOp) {
      i += matchedOp.length;
      tokens.push({ kind: "operator", text: matchedOp, start, end: i });
      continue;
    }

    if (isIdentStart(c)) {
      i = readWhile(input, i, isIdentPart);
      const text = input.slice(start, i);
      const lower = text.toLowerCase();
      let kind: TokenKind = "ident";
      if (TABLES.has(text)) kind = "table";
      else if (KEYWORDS.has(lower)) kind = "keyword";
      else if (FUNCTIONS.has(lower)) kind = "function";
      else if (FIELD_NAMES.has(text)) kind = "field";
      tokens.push({ kind, text, start, end: i });
      continue;
    }

    // Unrecognised character — classify as ident so the highlighter keeps rendering.
    i++;
    tokens.push({ kind: "ident", text: input.slice(start, i), start, end: i });
  }
  return tokens;
}
