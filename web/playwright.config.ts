import { defineConfig, devices } from "@playwright/test";

const STUB_PORT = 4010;
const APP_PORT = 3003;
/**
 * Where the stub answers the dashboard's wallet reads, one port per chain.
 * `just dev` runs Anvil on 8545 and 8546; `PW_RPC_PORTS=18545,18546` runs the
 * suite beside it.
 */
const RPC_PORTS = (process.env.PW_RPC_PORTS ?? "8545,8546").split(",");

/**
 * The checkout renders on the server, so intercepting requests in the browser
 * would miss the first paint entirely. Instead both the app and the browser
 * talk to a stub payer API, and the deposit request id selects the scenario. The same
 * stub plays the merchant API for the dashboard specs, and Privy itself is
 * replaced at bundle time by `test/privy-stub.tsx` (`GUM_PRIVY_STUB`).
 */
const publicEnv = {
  NEXT_PUBLIC_GUM_API_URL: `http://127.0.0.1:${STUB_PORT}`,
  NEXT_PUBLIC_PRIVY_APP_ID: "privy-stub-app",
  GUM_PRIVY_STUB: "1",
  NEXT_PUBLIC_CHAINS: JSON.stringify([
    {
      id: 143,
      name: "Monad",
      rpcUrl: `http://127.0.0.1:${RPC_PORTS[0]}`,
      tokens: [
        { currency: "USDC", address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603" },
        { currency: "USDT", address: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D" },
      ],
      explorerUrl: "https://monadvision.com",
      confirmation: "Credited within seconds",
      cctp: { domain: 15, forwarder: "0xF0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0", tokenMessenger: "0x3333333333333333333333333333333333333333", messageTransmitter: "0x4444444444444444444444444444444444444444" },
    },
    {
      id: 8453,
      name: "Base",
      rpcUrl: `http://127.0.0.1:${RPC_PORTS[1]}`,
      tokens: [{ currency: "USDC", address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" }],
      explorerUrl: "https://basescan.org",
      confirmation: "Credited within a minute",
      cctp: { domain: 6, forwarder: "0xF0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0", tokenMessenger: "0x6666666666666666666666666666666666666666", messageTransmitter: "0x7777777777777777777777777777777777777777" },
    },
  ]),
  NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID: "",
};

/**
 * `PW_TARGET=dev` runs the same specs against `next dev`.
 *
 * Development and production differ in ways that break one and not the other —
 * the dev server enforces an origin check on its own chunks, and only the
 * production build prerenders. A suite that only ever saw one of them shipped a
 * landing page whose scripts never loaded, so both are testable from here.
 */
const target = process.env.PW_TARGET === "dev" ? "dev" : "production";

const appServer =
  target === "dev"
    ? `next dev --port ${APP_PORT}`
    : `next build && next start --port ${APP_PORT}`;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  name: target,
  // The dev server compiles each route on the first hit, so a cold suite walk
  // (the docs specs visit every page) needs minutes on CI's two cores where
  // the production build serves instantly. Only the timeout differs; the
  // assertions stay the same for both targets.
  timeout: target === "dev" ? 180_000 : 30_000,
  use: {
    baseURL: `http://127.0.0.1:${APP_PORT}`,
    trace: "on-first-retry",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: [
    {
      command: "node e2e/stub-api.mjs",
      url: `http://127.0.0.1:${STUB_PORT}/health`,
      reuseExistingServer: !process.env.CI,
      env: {
        STUB_PORT: String(STUB_PORT),
        STUB_RPC_PORTS: RPC_PORTS.join(","),
        CHECKOUT_ORIGIN: `http://127.0.0.1:${APP_PORT}`,
      },
    },
    {
      // NEXT_PUBLIC_* values are inlined at build time, so the build has to
      // happen with the stub's origin rather than a developer's .env.local.
      command: appServer,
      url: `http://127.0.0.1:${APP_PORT}/`,
      reuseExistingServer: false,
      timeout: 180_000,
      env: publicEnv,
    },
  ],
});
