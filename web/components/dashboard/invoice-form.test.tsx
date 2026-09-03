import type { PaydayClient } from "@payday/sdk";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildCreatePayment, EMPTY_VALUES, InvoiceForm } from "./invoice-form";
import { MerchantProvider } from "./session";
import * as attachmentUpload from "@/lib/attachment-upload";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/dashboard/invoices/new",
}));

const CUSTOMER = {
  id: "0198f80c-8d2f-7dc1-a369-90556a64f7c1",
  name: "Globex Corporation",
  email: "ap@globex.example",
  details: "PO 7781",
  created_at: "2026-08-01T09:00:00Z",
  updated_at: "2026-08-01T09:00:00Z",
};

function renderForm() {
  const create = vi.fn().mockResolvedValue({ id: "pay_new" });
  const client = {
    customers: { list: vi.fn().mockResolvedValue({ customers: [CUSTOMER], next_cursor: null }) },
    payments: { create },
  } as unknown as PaydayClient;
  render(
    <MerchantProvider value={{ client, accessToken: "eyJ.dash.token", signOut: vi.fn() }}>
      <InvoiceForm />
    </MerchantProvider>,
  );
  return { create };
}

beforeEach(() => {
  push.mockReset();
});

describe("InvoiceForm policy modes", () => {
  it("cannot issue while an attachment is uploading or scanning", async () => {
    const user = userEvent.setup();
    let finish!: (value: null) => void;
    vi.spyOn(attachmentUpload, "uploadAttachment").mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const { create } = renderForm();
    const file = new File(["%PDF-1.4"], "invoice.pdf", { type: "application/pdf" });

    const uploading = user.upload(screen.getByLabelText("Attachment (PDF, up to 5 MiB)"), file);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Issue invoice" })).toBeDisabled(),
    );
    expect(create).not.toHaveBeenCalled();

    finish(null);
    await uploading;
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Issue invoice" })).toBeEnabled(),
    );
  });

  it("offers the four modes and asks for assertions only where the mode needs them", async () => {
    const user = userEvent.setup();
    renderForm();

    const modes = screen.getAllByRole("radio");
    expect(modes.map((radio) => (radio as HTMLInputElement).value)).toEqual([
      "permissionless",
      "verified_email",
      "verified_identity",
      "verified_identity_unattributed",
    ]);
    expect(screen.queryByLabelText("Expected payer email")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Expected first name")).not.toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: /^Verified email/ }));
    expect(screen.getByLabelText("Expected payer email")).toBeInTheDocument();
    expect(screen.queryByLabelText("Expected first name")).not.toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: /^Verified identity$/ }));
    expect(screen.getByLabelText("Expected payer email")).toBeInTheDocument();
    expect(screen.getByLabelText("Expected first name")).toBeInTheDocument();
    expect(screen.getByLabelText("Expected last name")).toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: /unattributed/ }));
    expect(screen.getByLabelText("Expected payer email")).toBeInTheDocument();
    expect(screen.queryByLabelText("Expected first name")).not.toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: /^Permissionless/ }));
    expect(screen.queryByLabelText("Expected payer email")).not.toBeInTheDocument();
  });

  it("pre-fills bill to from the chosen customer and sends the policy the API expects", async () => {
    const user = userEvent.setup();
    const { create } = renderForm();

    await user.type(screen.getByLabelText("Issuer name"), "Acme Corp");
    await user.selectOptions(await screen.findByLabelText("Customer"), CUSTOMER.id);
    expect(screen.getByLabelText("Bill to name")).toHaveValue("Globex Corporation");
    expect(screen.getByLabelText("Bill to email")).toHaveValue("ap@globex.example");
    expect(screen.getByLabelText("Bill to details")).toHaveValue("PO 7781");

    await user.type(screen.getByLabelText("Amount (USDC)"), "25");
    await user.type(
      screen.getByLabelText("Payout address"),
      "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    );
    await user.click(screen.getByRole("radio", { name: /^Verified email/ }));
    await user.type(screen.getByLabelText("Expected payer email"), "alice@globex.example");
    await user.click(screen.getByRole("button", { name: "Issue invoice" }));

    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const [body, idempotencyKey] = create.mock.calls[0] as [Record<string, unknown>, string];
    expect(body).toMatchObject({
      amount: "25",
      payout_address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
      issuer: { name: "Acme Corp" },
      bill_to: { name: "Globex Corporation", email: "ap@globex.example", details: "PO 7781" },
      customer_id: CUSTOMER.id,
      payer_policy: { mode: "verified_email", expected_email: "alice@globex.example" },
    });
    expect(body).not.toHaveProperty("attachment_id");
    expect(body).not.toHaveProperty("memo");
    expect(idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
    expect(push).toHaveBeenCalledWith("/dashboard/invoices/pay_new");
  });
});

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
});
