"use client";

import * as Tabs from "@radix-ui/react-tabs";
import { type ReactNode, useEffect, useState } from "react";
import { cn } from "@/lib/cn";

/**
 * The same sample in more than one language. Choosing a tab is remembered in
 * the browser and applied to every other tab group on the site, so a reader
 * who works in TypeScript sees TypeScript everywhere after picking it once.
 * The content itself is rendered on the server and handed in as nodes.
 */

const STORAGE_KEY = "payday.docs.lang";
const CHANGE_EVENT = "payday:docs-lang";

export interface CodeTab {
  label: string;
  content: ReactNode;
}

function readPreference(): string | null {
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

function writePreference(label: string) {
  try {
    window.localStorage.setItem(STORAGE_KEY, label);
  } catch {
    // Storage can be unavailable; the choice still applies to this page.
  }
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: label }));
}

export function CodeTabs({ tabs }: { tabs: CodeTab[] }) {
  const first = tabs[0]?.label ?? "";
  const [value, setValue] = useState(first);

  useEffect(() => {
    const apply = (label: string | null) => {
      if (label && tabs.some((tab) => tab.label === label)) setValue(label);
    };
    apply(readPreference());
    const onChange = (event: Event) => apply((event as CustomEvent<string>).detail);
    window.addEventListener(CHANGE_EVENT, onChange);
    return () => window.removeEventListener(CHANGE_EVENT, onChange);
  }, [tabs]);

  return (
    <Tabs.Root
      value={value}
      onValueChange={(next) => {
        setValue(next);
        writePreference(next);
      }}
      className="docs-tabs my-5"
    >
      <Tabs.List
        aria-label="Language"
        className="flex gap-1 rounded-t-[12px] border border-b-0 border-term-line bg-term-chrome px-2 pt-2"
      >
        {tabs.map((tab) => (
          <Tabs.Trigger
            key={tab.label}
            value={tab.label}
            className={cn(
              "rounded-t-[8px] px-3 py-1.5 font-mono text-[11px] tracking-[0.04em] uppercase transition-colors",
              "text-term-dim hover:text-term-text",
              "data-[state=active]:bg-term-bg data-[state=active]:text-term-text",
            )}
          >
            {tab.label}
          </Tabs.Trigger>
        ))}
      </Tabs.List>
      {tabs.map((tab) => (
        // `docs-tabs` in globals.css folds each sample's own chrome into the
        // tab bar: the label goes, the copy control moves up beside the tabs.
        <Tabs.Content key={tab.label} value={tab.label} className="relative">
          {tab.content}
        </Tabs.Content>
      ))}
    </Tabs.Root>
  );
}
