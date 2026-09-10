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
    // the configured chains or tokens means the same thing everywhere.
    env: {
      NEXT_PUBLIC_PAYDAY_API_URL: "https://api.example.test",
      NEXT_PUBLIC_CHAINS: JSON.stringify([
        {
          id: 143,
          name: "Monad",
          rpcUrl: "https://rpc.example.test",
          usdcAddress: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
          explorerUrl: "https://explorer.example.test",
        },
        {
          id: 8453,
          name: "Base",
          rpcUrl: "https://base.example.test",
          usdcAddress: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          explorerUrl: "https://base-explorer.example.test",
        },
      ]),
      NEXT_PUBLIC_PRIVY_APP_ID: "privy-test-app",
    },
  },
});
