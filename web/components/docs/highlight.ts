/**
 * A small tokenizer for the handful of languages the documentation shows.
 * Enough to colour strings, comments, keywords, and numbers the way the
 * landing page's terminal panels do; not a grammar. It runs on the server,
 * so nothing here reaches the browser bundle.
 */

export type Language = "bash" | "json" | "ts" | "http" | "text";

export type TokenType =
  | "plain"
  | "string"
  | "key"
  | "comment"
  | "keyword"
  | "number"
  | "variable"
  | "flag"
  | "method"
  | "header";

export interface Token {
  type: TokenType;
  text: string;
}

type Rule = [TokenType, RegExp];

const TS_KEYWORDS =
  "await|async|const|let|var|function|return|import|export|from|new|if|else|for|of|in|throw|try|catch|finally|class|interface|type|extends|implements|default|null|undefined|true|false|as|typeof|instanceof|switch|case|break|continue|while|do|void|yield|static|readonly|private|public|protected";

const BASH_KEYWORDS =
  "curl|export|echo|jq|npm|npx|node|cat|set|if|then|fi|for|do|done|while|openssl|base64|printf";

const RULES: Record<Language, Rule[]> = {
  json: [
    ["key", /"(?:[^"\\]|\\.)*"(?=\s*:)/y],
    ["string", /"(?:[^"\\]|\\.)*"/y],
    ["number", /-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/y],
    ["keyword", /\b(?:true|false|null)\b/y],
    ["plain", /[\s\S]/y],
  ],
  bash: [
    ["comment", /#[^\n]*/y],
    ["string", /'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"/y],
    ["variable", /\$\{[^}]+\}|\$[A-Za-z_][A-Za-z0-9_]*|\$\([^)]*\)/y],
    ["flag", /(?<![\w-])--?[A-Za-z][\w-]*/y],
    ["keyword", new RegExp(`\\b(?:${BASH_KEYWORDS})\\b`, "y")],
    ["number", /\b\d+(?:\.\d+)?\b/y],
    ["plain", /[\s\S]/y],
  ],
  ts: [
    ["comment", /\/\/[^\n]*|\/\*[\s\S]*?\*\//y],
    ["string", /`(?:[^`\\]|\\.)*`|'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"/y],
    ["keyword", new RegExp(`\\b(?:${TS_KEYWORDS})\\b`, "y")],
    ["number", /\b\d+(?:\.\d+)?n?\b/y],
    ["plain", /[\s\S]/y],
  ],
  http: [
    ["method", /^(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/my],
    ["header", /^[A-Za-z][A-Za-z0-9-]*(?=:)/my],
    ["comment", /#[^\n]*/y],
    ["number", /\b\d{3}\b/y],
    ["plain", /[\s\S]/y],
  ],
  text: [["plain", /[\s\S]+/y]],
};

export function tokenize(code: string, language: Language): Token[] {
  const rules = RULES[language];
  const tokens: Token[] = [];
  let position = 0;

  while (position < code.length) {
    let matched = false;
    for (const [type, pattern] of rules) {
      pattern.lastIndex = position;
      const match = pattern.exec(code);
      if (!match || match.index !== position || match[0].length === 0) continue;
      const text = match[0];
      const last = tokens[tokens.length - 1];
      if (last && last.type === "plain" && type === "plain") {
        last.text += text;
      } else {
        tokens.push({ type, text });
      }
      position += text.length;
      matched = true;
      break;
    }
    if (!matched) {
      // Every language ends with a catch-all, so this is only defensive.
      tokens.push({ type: "plain", text: code.charAt(position) });
      position += 1;
    }
  }

  return tokens;
}
