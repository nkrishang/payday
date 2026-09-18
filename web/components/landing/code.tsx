import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The pieces the landing page's code pictures are made of: an editor
 * panel, enough of a highlighter for a dozen lines, and the marks of a
 * currency and a chain.
 */

/** An editor panel: a file's name up top, its lines, and a footer for what came back. */
export function Editor({
  file,
  footer,
  children,
}: {
  file: string;
  footer: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex h-full flex-col overflow-hidden rounded-[14px] border border-gum-white/[0.08] bg-gum-white/[0.04]">
      <div className="flex items-center gap-3 border-b border-gum-white/[0.08] px-4 py-2.5 font-mono text-[12.5px]">
        <span className="flex gap-1.5" aria-hidden="true">
          <span className="size-2.5 rounded-full bg-gum-white/15" />
          <span className="size-2.5 rounded-full bg-gum-white/15" />
          <span className="size-2.5 rounded-full bg-gum-white/15" />
        </span>
        <span className="text-gum-white">{file}</span>
        <span className="ml-auto text-gum-white/55">your backend</span>
      </div>
      <pre className="min-h-0 flex-1 overflow-hidden px-5 py-4 font-mono text-[14px] leading-[1.6] tracking-[-0.02em] text-gum-white/85">
        {children}
      </pre>
      <div className="flex min-h-[46px] items-center gap-3 border-t border-gum-white/[0.08] px-5 font-mono text-[13px]">
        {footer}
      </div>
    </div>
  );
}

/** Enough of a highlighter for a dozen lines: strings, keywords, calls, punctuation. */
export function Highlighted({ code }: { code: string }) {
  const pattern =
    /("(?:[^"\\]|\\.)*"?)|\b(const|await|async|if|return)\b|([A-Za-z_$][\w$]*)(?=\()|([{}()[\],;.]|=>|===|=)|([A-Za-z_$][\w$]*)|(\d+(?:\.\d+)?)|(\s+)|(.)/g;
  const parts: ReactNode[] = [];
  for (const match of code.matchAll(pattern)) {
    const [text, string, keyword, call, punctuation, word] = match;
    const key = parts.length;
    if (string) {
      parts.push(
        <span key={key} className="text-gum-pink">
          {text}
        </span>,
      );
    } else if (keyword) {
      parts.push(
        <span key={key} className="text-gum-white">
          {text}
        </span>,
      );
    } else if (call) {
      parts.push(
        <span key={key} className="text-gum-white">
          {text}
        </span>,
      );
    } else if (punctuation) {
      parts.push(
        <span key={key} className="text-gum-white/55">
          {text}
        </span>,
      );
    } else if (word) {
      parts.push(<span key={key}>{text}</span>);
    } else {
      parts.push(text);
    }
  }
  return <>{parts}</>;
}

export function Usdc({ className }: { className?: string }) {
  return (
    <Image
      src="/payment-icons/usdc.svg"
      width={64}
      height={64}
      loading="eager"
      alt=""
      className={cn("shrink-0 rounded-full", className)}
    />
  );
}

export function Monad({ className }: { className?: string }) {
  return (
    <Image
      src="/payment-icons/monad.svg"
      width={64}
      height={64}
      loading="eager"
      alt=""
      className={cn("shrink-0", className)}
    />
  );
}
