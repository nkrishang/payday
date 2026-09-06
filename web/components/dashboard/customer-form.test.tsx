import type { Customer, PaydayClient } from "@payday/sdk";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CustomerForm } from "./customer-form";
import { MerchantProvider } from "./session";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace: vi.fn() }),
}));

const CUSTOMER: Customer = {
  id: "cus_0198f80c-8d2f-7dc1-a369-90556a64f7c1",
  name: "Globex Corporation",
  email: "ap@globex.example",
  details: "PO 7781",
  created_at: "2026-08-01T09:00:00Z",
  updated_at: "2026-08-01T09:00:00Z",
};

function renderForm(props: { customer?: Customer; onSaved?: (customer: Customer) => void } = {}) {
  const create = vi.fn().mockResolvedValue({ ...CUSTOMER, id: "cus_0198f80c-0000-7dc1-a369-90556a64f7c2" });
  const update = vi.fn().mockResolvedValue({ ...CUSTOMER, email: null, details: null });
  const client = { customers: { create, update } } as unknown as PaydayClient;
  render(
    <MerchantProvider
      value={{ client, accessToken: "eyJ.dash.token", email: "merchant@example.com", signOut: vi.fn() }}
    >
      <CustomerForm {...props} />
    </MerchantProvider>,
  );
  return { create, update };
}

beforeEach(() => {
  push.mockReset();
});

describe("CustomerForm create", () => {
  it("sends only the trimmed name when email and details are left blank", async () => {
    const user = userEvent.setup();
    const { create } = renderForm();

    await user.type(screen.getByLabelText("Name"), "  Globex Corporation  ");
    await user.click(screen.getByRole("button", { name: "Create customer" }));

    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    // A blank optional is omitted rather than sent as "", so the API stores
    // no email or details instead of rejecting an empty string.
    expect(create.mock.calls[0]![0]).toEqual({ name: "Globex Corporation" });
    expect(push).toHaveBeenCalledWith("/dashboard/customers/cus_0198f80c-0000-7dc1-a369-90556a64f7c2");
  });

  it("flags a blank name in the form and never calls the API", async () => {
    const user = userEvent.setup();
    const { create } = renderForm();

    await user.click(screen.getByRole("button", { name: "Create customer" }));

    expect(screen.getByLabelText("Name")).toBeInvalid();
    expect(create).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
  });
});

describe("CustomerForm update", () => {
  it("sends null for a cleared email and details so the replacement clears them", async () => {
    const user = userEvent.setup();
    const onSaved = vi.fn();
    const { update } = renderForm({ customer: CUSTOMER, onSaved });

    expect(screen.getByLabelText("Email")).toHaveValue(CUSTOMER.email);
    expect(screen.getByLabelText("Details")).toHaveValue(CUSTOMER.details);
    await user.clear(screen.getByLabelText("Email"));
    await user.clear(screen.getByLabelText("Details"));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    // PATCH is a full replacement, so an emptied field must travel as null;
    // omitting it would leave the old value in place.
    expect(update).toHaveBeenCalledWith(CUSTOMER.id, {
      name: CUSTOMER.name,
      email: null,
      details: null,
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Saved");
    expect(onSaved).toHaveBeenCalledWith({ ...CUSTOMER, email: null, details: null });
    expect(push).not.toHaveBeenCalled();
  });

  it("disables Save until a field actually differs from what is stored", async () => {
    const user = userEvent.setup();
    renderForm({ customer: CUSTOMER });

    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();

    await user.type(screen.getByLabelText("Name"), " Inc.");
    expect(save).toBeEnabled();

    await user.type(screen.getByLabelText("Name"), "{backspace}{backspace}{backspace}{backspace}{backspace}");
    expect(save).toBeDisabled();
  });
});
