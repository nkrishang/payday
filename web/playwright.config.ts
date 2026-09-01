import { defineConfig, devices } from "@playwright/test";

const STUB_PORT = 4010;
const APP_PORT = 3003;

/**
 * The checkout renders on the server, so intercepting requests in the browser
 * would miss the first paint entirely. Instead both the app and the browser
 * talk to a stub payer API, and the payment id selects the scenario.
 */
const publicEnv = {
  NEXT_PUBLIC_PAYDAY_API_URL: `http://127.0.0.1:${STUB_PORT}`,
  NEXT_PUBLIC_CHAIN_ID: "143",
  NEXT_PUBLIC_CHAIN_NAME: "Monad",
  NEXT_PUBLIC_RPC_URL: "http://127.0.0.1:8545",
  NEXT_PUBLIC_USDC_ADDRESS: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  NEXT_PUBLIC_EXPLORER_BASE_URL: "https://monadvision.com",
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
      env: { STUB_PORT: String(STUB_PORT) },
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
