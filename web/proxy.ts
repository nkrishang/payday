import { NextResponse, type NextRequest } from "next/server";

/**
 * Content-Security-Policy.
 *
 * The checkout renders per request, so it gets a nonce: Next reads it back off
 * the request header and stamps it onto its own bootstrap scripts, and
 * `script-src` needs no `unsafe-inline`. That is the page that holds a live pay
 * button, and it is where the strictness is worth having.
 *
 * The rest of the site is statically prerendered and cacheable, and a nonce
 * minted per request can never match HTML baked at build time — presenting one
 * there blocks every script on the page. Those routes get the same policy with
 * `unsafe-inline` for scripts instead, which is sound because they render no
 * dynamic content at all.
 */

/**
 * The dashboard renders per request too: it holds a merchant's bearer token in
 * memory and is never prerendered, so it is nonced for the same reason.
 */
const NONCED_PATHS = ["/pay/", "/dashboard"];

const WALLETCONNECT = [
  "https://*.walletconnect.com",
  "https://*.walletconnect.org",
  "wss://*.walletconnect.com",
  "wss://*.walletconnect.org",
  "https://*.reown.com",
  "wss://*.reown.com",
];

function origin(url: string | undefined): string[] {
  if (!url) return [];
  try {
    return [new URL(url).origin];
  } catch {
    return [];
  }
}

/** Every configured chain's public RPC origin: the wallet reads through each. */
function rpcOrigins(raw: string | undefined): string[] {
  if (!raw) return [];
  try {
    const chains = JSON.parse(raw) as Array<{ rpcUrl?: unknown }>;
    return chains.flatMap((chain) =>
      typeof chain.rpcUrl === "string" ? origin(chain.rpcUrl) : [],
    );
  } catch {
    return [];
  }
}

/**
 * Merchant sign-in and the embedded wallet are Privy's. Its SDK talks to
 * `auth.privy.io`, hosts the wallet's key material in an iframe from the
 * same origin, reads chains through its own RPC proxy, and may present a
 * Cloudflare Turnstile challenge — all of which the policy names, per
 * Privy's published CSP guidance. A deployment with a Privy base domain
 * would add that origin here too.
 */
const PRIVY_CONNECT = ["https://auth.privy.io", "https://*.rpc.privy.systems"];
const PRIVY_FRAMES = ["https://auth.privy.io", "https://challenges.cloudflare.com"];
const PRIVY_SCRIPTS = ["https://challenges.cloudflare.com"];

function policy(nonce: string | null, isDev: boolean): string {
  const api = origin(process.env.NEXT_PUBLIC_PAYDAY_API_URL);
  const connect = [
    "'self'",
    ...api,
    ...rpcOrigins(process.env.NEXT_PUBLIC_CHAINS),
    ...WALLETCONNECT,
    ...PRIVY_CONNECT,
    // The dashboard PUTs attachment bytes to the presigned upload origin
    // directly from the browser.
    ...origin(process.env.NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN),
  ];

  // 'strict-dynamic' lets the nonced bootstrap load Next's chunks; dev needs
  // eval for React Refresh.
  const script = nonce
    ? `'self' 'nonce-${nonce}' 'strict-dynamic'${isDev ? " 'unsafe-eval'" : ""}`
    : `'self' 'unsafe-inline'${isDev ? " 'unsafe-eval'" : ""}`;

  return [
    "default-src 'self'",
    `script-src ${script} ${PRIVY_SCRIPTS.join(" ")}`,
    // React writes style attributes (the received-amount bar), and those are
    // covered by style-src in browsers without style-src-attr support.
    "style-src 'self' 'unsafe-inline'",
    // Wallet icons arrive as data URIs over EIP-6963; the deposit QR is an SVG
    // served by the API, which is a plain-HTTP loopback origin in development
    // and so is named explicitly rather than covered by `https:`.
    `img-src 'self' data: blob: https: ${api.join(" ")}`.trimEnd(),
    "font-src 'self'",
    `connect-src ${connect.join(" ")}`,
    `frame-src 'self' ${PRIVY_FRAMES.join(" ")}`,
    `child-src 'self' ${PRIVY_FRAMES.join(" ")}`,
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "frame-ancestors 'none'",
    // Loopback services are plain HTTP in development; upgrading them breaks
    // the QR and every read.
    ...(isDev ? [] : ["upgrade-insecure-requests"]),
  ].join("; ");
}

export default function proxy(request: NextRequest) {
  const isDev = process.env.NODE_ENV !== "production";
  const usesNonce = NONCED_PATHS.some((path) => request.nextUrl.pathname.startsWith(path));

  let nonce: string | null = null;
  if (usesNonce) {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    nonce = btoa(String.fromCharCode(...bytes));
  }

  const csp = policy(nonce, isDev);
  const requestHeaders = new Headers(request.headers);
  if (nonce) {
    requestHeaders.set("x-nonce", nonce);
    requestHeaders.set("content-security-policy", csp);
  }

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set("content-security-policy", csp);
  return response;
}

export const config = {
  matcher: [
    // Everything except Next's own endpoints and files with an extension.
    // The whole of /_next/ is excluded, not just static and image: /_next/hmr
    // is a websocket upgrade in development, and running it through here
    // breaks the handshake and with it fast refresh.
    {
      source: "/((?!_next/|favicon.ico|.*\\..*).*)",
      missing: [
        { type: "header", key: "next-router-prefetch" },
        { type: "header", key: "purpose", value: "prefetch" },
      ],
    },
  ],
};
