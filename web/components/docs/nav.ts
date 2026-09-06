import { API_GROUPS } from "./api";
import type { Method } from "./api/types";

/**
 * The documentation's two tables of contents: the guides, and the API
 * reference. Each is one list, read by the sidebar, the previous/next pager
 * at the foot of every page, the landing cards, and the browser suite — so a
 * page added here appears everywhere at once. The reference's endpoint
 * entries are derived from the endpoint records in `./api`, never listed by
 * hand.
 */

export interface DocsPageEntry {
  href: string;
  title: string;
  /** One line under the title on the landing cards and in the pager. */
  summary?: string;
  /** Shown as a badge before the title in the reference sidebar. */
  method?: Method;
}

export interface DocsGroup {
  title: string;
  pages: DocsPageEntry[];
}

export type DocsSection = "docs" | "api";

export const DOCS_NAV: ReadonlyArray<DocsGroup> = [
  {
    title: "Getting started",
    pages: [
      {
        href: "/docs",
        title: "Introduction",
        summary: "What Payday does and how a deposit flows.",
      },
      {
        href: "/docs/quickstart",
        title: "Quickstart",
        summary: "Mint a key, issue a request, share the link, watch it settle.",
      },
      {
        href: "/docs/concepts",
        title: "Deposit requests and deposits",
        summary: "The two primitives, the lifecycle, and where every unit of USDC goes.",
      },
      {
        href: "/docs/payer-verification",
        title: "Verifying the payer",
        summary: "Three payer policies and the wallet step every request takes.",
      },
    ],
  },
  {
    title: "Using Payday",
    pages: [
      {
        href: "/docs/dashboard",
        title: "Dashboard",
        summary: "Issue and track deposit requests without writing code.",
      },
      {
        href: "/docs/checkout",
        title: "Hosted checkout",
        summary: "What the payer sees, and how to build your own.",
      },
      {
        href: "/docs/webhooks",
        title: "Webhooks",
        summary: "Events, signatures, payloads, and retries.",
      },
      {
        href: "/docs/proof-of-payment",
        title: "Proof of Payment",
        summary: "An offline-verifiable record of who paid what, where.",
      },
      {
        href: "/docs/recipes",
        title: "Recipes",
        summary: "Complete integration patterns, end to end.",
      },
      {
        href: "/docs/sdk",
        title: "TypeScript SDK",
        summary: "A zero-dependency client for Node and the browser.",
      },
      {
        href: "/docs/environments",
        title: "Environments",
        summary: "Production, sandbox, and the chain each one settles on.",
      },
    ],
  },
  {
    title: "Trust",
    pages: [
      {
        href: "/docs/security",
        title: "How Payday works",
        summary: "Architecture and security, for due diligence.",
      },
      { href: "/docs/faq", title: "FAQ", summary: "Short answers to the questions merchants ask." },
    ],
  },
];

export const API_NAV: ReadonlyArray<DocsGroup> = [
  {
    title: "API reference",
    pages: [
      {
        href: "/docs/api",
        title: "Introduction",
        summary: "Base URLs, authentication, ids, errors, paging, limits.",
      },
      {
        href: "/docs/api/errors",
        title: "Errors",
        summary: "Every stable error code and what to do about it.",
      },
    ],
  },
  ...API_GROUPS.map((group) => ({
    title: group.title,
    pages: [
      ...(group.overview ? [{ href: `/docs/api/${group.slug}`, title: group.overview.title }] : []),
      ...group.endpoints.map((endpoint) => ({
        href: `/docs/api/${group.slug}/${endpoint.slug}`,
        title: endpoint.title,
        method: endpoint.method,
      })),
    ],
  })),
];

export const DOCS_PAGES: ReadonlyArray<DocsPageEntry> = DOCS_NAV.flatMap((group) => group.pages);
export const API_PAGES: ReadonlyArray<DocsPageEntry> = API_NAV.flatMap((group) => group.pages);

export function sectionOf(pathname: string): DocsSection {
  return pathname === "/docs/api" || pathname.startsWith("/docs/api/") ? "api" : "docs";
}

export function navFor(section: DocsSection): ReadonlyArray<DocsGroup> {
  return section === "api" ? API_NAV : DOCS_NAV;
}

export function findPage(href: string): DocsPageEntry | undefined {
  return [...DOCS_PAGES, ...API_PAGES].find((page) => page.href === href);
}

export function neighbours(href: string): { previous?: DocsPageEntry; next?: DocsPageEntry } {
  const pages = sectionOf(href) === "api" ? API_PAGES : DOCS_PAGES;
  const index = pages.findIndex((page) => page.href === href);
  if (index === -1) return {};
  const previous = pages[index - 1];
  const next = pages[index + 1];
  return {
    ...(previous ? { previous } : {}),
    ...(next ? { next } : {}),
  };
}
