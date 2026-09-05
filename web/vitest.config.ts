import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./", import.meta.url)),
      // Component tests never sign in for real; the stub keeps the same
      // hooks and the same shapes without loading Privy's SDK into jsdom.
      "@privy-io/react-auth": fileURLToPath(new URL("./test/privy-stub.tsx", import.meta.url)),
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./test/setup.ts"],
    include: ["{lib,components,app}/**/*.test.{ts,tsx}"],
    // Fixed values rather than a developer's .env.local, so a test asserting on
    // the configured chain or token means the same thing everywhere.
    env: {
      NEXT_PUBLIC_PAYDAY_API_URL: "https://api.example.test",
      NEXT_PUBLIC_CHAIN_ID: "143",
      NEXT_PUBLIC_CHAIN_NAME: "Monad",
      NEXT_PUBLIC_RPC_URL: "https://rpc.example.test",
      NEXT_PUBLIC_USDC_ADDRESS: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
      NEXT_PUBLIC_EXPLORER_BASE_URL: "https://explorer.example.test",
      NEXT_PUBLIC_PRIVY_APP_ID: "privy-test-app",
    },
  },
});
