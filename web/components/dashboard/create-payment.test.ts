import { describe, expect, it } from "vitest";
import { buildCreatePayment, EMPTY_VALUES } from "./create-payment";

describe("buildCreatePayment", () => {
  const filled = {
    ...EMPTY_VALUES,
    issuerName: " Acme Corp ",
    billName: "Globex",
    amount: "25.5",
    payoutAddress: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    expectedEmail: "Alice@Example.com",
    firstName: "Alice",
    lastName: "Smith",
  };

  it("builds each of the four policy shapes without stray fields", () => {
    expect(buildCreatePayment({ ...filled, mode: "permissionless" }, null).payer_policy).toEqual({
      mode: "permissionless",
    });
    expect(buildCreatePayment({ ...filled, mode: "verified_email" }, null).payer_policy).toEqual({
      mode: "verified_email",
      expected_email: "Alice@Example.com",
    });
    expect(buildCreatePayment({ ...filled, mode: "verified_identity" }, null).payer_policy).toEqual(
      {
        mode: "verified_identity",
        expected_email: "Alice@Example.com",
        expected_identity: { first_name: "Alice", last_name: "Smith" },
      },
    );
    expect(
      buildCreatePayment({ ...filled, mode: "verified_identity_unattributed" }, null).payer_policy,
    ).toEqual({ mode: "verified_identity_unattributed", expected_email: "Alice@Example.com" });
  });

  it("omits blank optionals, trims parties, and converts hours to seconds", () => {
    const body = buildCreatePayment(
      { ...filled, heading: "  ", reference: "INV-1", expiresInHours: "48" },
      "0198f80c-8d2f-7dc1-a369-90556a64f7aa",
    );
    expect(body.issuer).toEqual({ name: "Acme Corp" });
    expect(body).not.toHaveProperty("heading");
    expect(body).not.toHaveProperty("notes");
    expect(body.reference).toBe("INV-1");
    expect(body.expires_in).toBe(172_800);
    expect(body.attachment_id).toBe("0198f80c-8d2f-7dc1-a369-90556a64f7aa");
  });

  it("sends a chosen moment as a moment, in place of any duration", () => {
    const body = buildCreatePayment(
      { ...filled, expiresInHours: "48", expiresAt: "2026-09-11T17:30:00.000Z" },
      null,
    );
    expect(body.expires_at).toBe("2026-09-11T17:30:00.000Z");
    expect(body).not.toHaveProperty("expires_in");
  });
});
