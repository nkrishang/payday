/**
 * The documentation's table of contents. One list, read by the sidebar, the
 * previous/next pager at the foot of every page, and the landing cards on
 * `/docs` — so a page added here appears everywhere at once.
 */

export interface DocsPageEntry {
  href: string;
  title: string;
  /** One line under the title on the landing cards and in the pager. */
  summary: string;
}

export interface DocsGroup {
  title: string;
  pages: DocsPageEntry[];
}

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
    title: "API reference",
    pages: [
      {
        href: "/docs/api",
        title: "Conventions",
        summary: "Authentication, ids, errors, paging, limits.",
      },
      {
        href: "/docs/api/deposit-requests",
        title: "Deposit requests",
        summary: "Create, read, list, cancel, transfers, documents, proof.",
      },
      { href: "/docs/api/customers", title: "Customers", summary: "Reusable payer records." },
      {
        href: "/docs/api/issuers",
        title: "Issuers and payout addresses",
        summary: "The party you issue under and the wallets you settle to.",
      },
      {
        href: "/docs/api/attachments",
        title: "Attachments",
        summary: "One scanned PDF per request.",
      },
      {
        href: "/docs/api/webhooks",
        title: "Webhooks",
        summary: "Endpoints, tests, and delivery history.",
      },
      {
        href: "/docs/api/account",
        title: "Account and status",
        summary: "Your account, its key, and service health.",
      },
      {
        href: "/docs/api/payer",
        title: "Payer routes",
        summary: "The public, keyless routes behind every deposit link.",
      },
      {
        href: "/docs/api/errors",
        title: "Errors",
        summary: "Every stable error code and what to do about it.",
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

export const DOCS_PAGES: ReadonlyArray<DocsPageEntry> = DOCS_NAV.flatMap((group) => group.pages);

export function findPage(href: string): DocsPageEntry | undefined {
  return DOCS_PAGES.find((page) => page.href === href);
}

export function neighbours(href: string): { previous?: DocsPageEntry; next?: DocsPageEntry } {
  const index = DOCS_PAGES.findIndex((page) => page.href === href);
  if (index === -1) return {};
  const previous = DOCS_PAGES[index - 1];
  const next = DOCS_PAGES[index + 1];
  return {
    ...(previous ? { previous } : {}),
    ...(next ? { next } : {}),
  };
}
