import { describe, expect, it } from "vitest";
import { buildCreateDepositRequest, EMPTY_VALUES } from "./create-deposit-request";

describe("buildCreateDepositRequest", () => {
  const filled = {
    ...EMPTY_VALUES,
    issuerName: " Acme Corp ",
    billName: "Globex",
    amount: "25.5",
    payoutAddress: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
  };

  it("builds each add-on shape without stray fields", () => {
    expect(buildCreateDepositRequest(filled, null).verification).toBeUndefined();
    expect(
      buildCreateDepositRequest(
        { ...filled, verification: { verifyEmail: true, expectedEmail: " Alice@Example.com ", walletAttestation: false } },
        null,
      ).verification,
    ).toEqual({ email: { expected_email: "Alice@Example.com" } });
    expect(
      buildCreateDepositRequest(
        { ...filled, verification: { verifyEmail: true, expectedEmail: "Alice@Example.com", walletAttestation: true } },
        null,
      ).verification,
    ).toEqual({ email: { expected_email: "Alice@Example.com" }, wallet_attestation: true });
    expect(
      buildCreateDepositRequest(
        { ...filled, verification: { verifyEmail: false, expectedEmail: "", walletAttestation: true } },
        null,
      ).verification,
    ).toEqual({ wallet_attestation: true });
  });

  it("never composes a merchant_auth add-on: only the merchant's app can open one", () => {
    const body = buildCreateDepositRequest(
      { ...filled, verification: { verifyEmail: true, expectedEmail: "alice@example.com", walletAttestation: true } },
      null,
    );
    expect(body.verification).not.toHaveProperty("merchant_auth");
  });

  it("omits blank optionals, trims parties, and converts hours to seconds", () => {
    const body = buildCreateDepositRequest(
      { ...filled, heading: "  ", reference: "INV-1", expiresInHours: "48" },
      "att_0198f80c-8d2f-7dc1-a369-90556a64f7aa",
    );
    expect(body.issuer).toEqual({ name: "Acme Corp" });
    expect(body).not.toHaveProperty("heading");
    expect(body).not.toHaveProperty("notes");
    expect(body.reference).toBe("INV-1");
    expect(body.expires_in).toBe(172_800);
    expect(body.attachment_id).toBe("att_0198f80c-8d2f-7dc1-a369-90556a64f7aa");
  });

  it("pins the network only when one was chosen", () => {
    expect(buildCreateDepositRequest(filled, null)).not.toHaveProperty("chain_id");
    expect(buildCreateDepositRequest({ ...filled, chainId: " 8453 " }, null).chain_id).toBe("8453");
  });

  it("sends a chosen moment as a moment, in place of any duration", () => {
    const body = buildCreateDepositRequest(
      { ...filled, expiresInHours: "48", expiresAt: "2026-09-11T17:30:00.000Z" },
      null,
    );
    expect(body.expires_at).toBe("2026-09-11T17:30:00.000Z");
    expect(body).not.toHaveProperty("expires_in");
  });
});
