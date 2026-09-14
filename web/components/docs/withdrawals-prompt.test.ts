import { describe, expect, it } from "vitest";
import {
  CHECKLIST,
  LEG_EXAMPLE,
  VECTOR_DIGEST,
  VECTOR_NONCE,
  WITHDRAW_GO,
  WITHDRAW_RUST,
  WITHDRAW_TS,
  WITHDRAWALS_PROMPT,
} from "./withdrawals-prompt";

/**
 * The prompt a reader hands their agent is built from the page's own
 * snippets and vectors, so it cannot drift from what the page shows or from
 * what gateway-core pins.
 */
describe("the withdrawals prompt", () => {
  it("carries every snippet the page shows, verbatim", () => {
    for (const snippet of [WITHDRAW_TS, WITHDRAW_RUST, WITHDRAW_GO, LEG_EXAMPLE]) {
      expect(WITHDRAWALS_PROMPT).toContain(snippet);
    }
  });

  it("carries the checklist and the shared test vector", () => {
    for (const item of CHECKLIST) expect(WITHDRAWALS_PROMPT).toContain(item);
    expect(WITHDRAWALS_PROMPT).toContain(VECTOR_DIGEST);
    expect(WITHDRAWALS_PROMPT).toContain(VECTOR_NONCE);
    expect(LEG_EXAMPLE).toContain(VECTOR_NONCE);
    // The digest gateway-core, the Solidity test, the SDK and the web adapter pin.
    expect(VECTOR_DIGEST).toBe("0xfe0bcc7d9e69ee02881011f29a4156caa15e02b8e94c2e0c9f40e66711651993");
  });

  it("names the four steps and the routes", () => {
    for (const route of [
      "POST https://api.payday.sh/v1/withdrawals",
      "/v1/withdrawals/{id}/authorizations",
      "GET /v1/withdrawals/{id}",
      "/v1/withdrawals/{id}/cancel",
    ]) {
      expect(WITHDRAWALS_PROMPT).toContain(route);
    }
  });
});
