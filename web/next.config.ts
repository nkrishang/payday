import type { NextConfig } from "next";

/**
 * Security headers applied to every response. The checkout holds a live "pay
 * now" button, so it must never be framable, and the payer's link should not
 * leak to third parties through a Referer header.
 *
 * Content-Security-Policy is set per-request in middleware.ts, where a nonce
 * is available.
 */
const securityHeaders = [
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "X-Frame-Options", value: "DENY" },
  { key: "Referrer-Policy", value: "no-referrer" },
  { key: "Cross-Origin-Opener-Policy", value: "same-origin" },
  {
    key: "Permissions-Policy",
    value: "camera=(), microphone=(), geolocation=(), interest-cohort=()",
  },
];

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // Next emits `crossorigin` on its own script tags, so the browser sends an
  // Origin header even same-origin, and the dev server answers 403 for any
  // origin it does not trust. It trusts localhost but not the literal loopback
  // address, which is what `just web` and PAYDAY_PUBLIC_BASE_URL both use —
  // without this, dev serves HTML that never hydrates.
  allowedDevOrigins: ["127.0.0.1", "localhost", "[::1]"],
  // This repo documents itself in docs/ and CLAUDE-less conventions; Next's
  // generated agent files would be unowned noise.
  agentRules: false,
  poweredByHeader: false,
  async headers() {
    return [{ source: "/:path*", headers: securityHeaders }];
  },
};

export default nextConfig;
