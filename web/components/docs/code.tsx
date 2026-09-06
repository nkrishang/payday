import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { type Language, type TokenType, tokenize } from "./highlight";

/**
 * A code sample in the landing page's terminal clothes: dark in both colour
 * schemes, a chrome bar naming what it is, and a copy control. Highlighting
 * happens on the server, so the browser receives coloured spans and no
 * tokenizer.
 */

const TOKEN_CLASS: Record<TokenType, string | undefined> = {
  plain: undefined,
  string: "text-term-accent",
  key: "text-term-id",
  comment: "text-term-dim italic",
  keyword: "text-[#c4b5fd]",
  number: "text-term-warn",
  variable: "text-term-warn",
  flag: "text-term-id",
  method: "text-term-accent font-semibold",
  header: "text-term-id",
};

const LANGUAGE_LABEL: Record<Language, string> = {
  bash: "Shell",
  json: "JSON",
  ts: "TypeScript",
  http: "HTTP",
  text: "Text",
};

export function CodeBlock({
  code,
  lang = "text",
  title,
  className,
}: {
  code: string;
  lang?: Language;
  /** Shown in the chrome bar in place of the language name. */
  title?: string;
  className?: string;
}) {
  const source = code.replace(/^\n+|\n+$/g, "");
  const tokens = tokenize(source, lang);

  return (
    <figure
      className={cn(
        "docs-code my-5 overflow-hidden rounded-[12px] border border-term-line bg-term-bg text-[13px] text-term-text",
        className,
      )}
    >
      <figcaption className="flex h-9 items-center justify-between gap-3 border-b border-term-line bg-term-chrome pr-1 pl-4">
        <span className="font-mono text-[11px] tracking-[0.04em] text-term-dim uppercase">
          {title ?? LANGUAGE_LABEL[lang]}
        </span>
        <CopyButton
          value={source}
          label="code"
          className="text-term-dim hover:bg-white/[0.06] hover:text-term-text"
        />
      </figcaption>
      <pre className="overflow-x-auto px-4 py-4 leading-[1.65]">
        <code className="font-mono">
          {tokens.map((token, index) =>
            TOKEN_CLASS[token.type] ? (
              <span key={index} className={TOKEN_CLASS[token.type]}>
                {token.text}
              </span>
            ) : (
              token.text
            ),
          )}
        </code>
      </pre>
    </figure>
  );
}
