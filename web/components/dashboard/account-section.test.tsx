import { type AccountMetadata, type GumClient } from "@gum/sdk";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MerchantProvider } from "./session";
import { AccountSection } from "./account-section";

/**
 * Changing the sign-in email: a code to the new mailbox, the address changed
 * only once that code confirms, and a wrong or expired code reported in
 * place — the page never moves on, and never reports success for a flow
 * Privy refused.
 */

const SESSION_KEY = "gum.privy-stub.session";

/** The shape the stub keeps in sessionStorage; the token is never parsed here. */
function signInAs(email: string) {
  sessionStorage.setItem(
    SESSION_KEY,
    JSON.stringify({
      email,
      sub: `did:privy:stub-${email.replace(/[^a-z]/g, "")}`,
      wallet: "0x2222222222222222222222222222222222222222",
      identityToken: "stub-dashboard-token.unused",
    }),
  );
}

function clientWithoutRoutes(): GumClient {
  // The section's only reads are the withdrawal list; every other route is
  // out of reach of a keyless section render, and the test never signs.
  return {
    withdrawals: { list: async () => ({ withdrawals: [], next_cursor: null }) },
  } as unknown as GumClient;
}

function renderSection(onChanged: () => void) {
  return render(
    <StrictMode>
      <MerchantProvider
        value={{
          client: clientWithoutRoutes(),
          accessToken: "stub-dashboard-token.unused",
          email: "merchant@example.com",
          subject: "did:privy:stub-merchantexamplecom",
          signOut: vi.fn(),
        }}
      >
        <AccountSection
          account={{ email: "merchant@example.com" } as AccountMetadata}
          onChanged={onChanged}
        />
      </MerchantProvider>
    </StrictMode>,
  );
}

beforeEach(() => {
  sessionStorage.clear();
});

describe("changing the sign-in email", () => {
  it("a wrong code is reported in place and the address does not change", async () => {
    const user = userEvent.setup();
    const onChanged = vi.fn();
    signInAs("merchant@example.com");
    renderSection(onChanged);

    await user.click(screen.getByRole("button", { name: "Change" }));
    const input = screen.getByLabelText("New email address");
    await user.type(input, "new@example.com");
    await user.click(screen.getByRole("button", { name: "Email a code" }));

    const code = await screen.findByLabelText("Verification code");
    await user.type(code, "999999");
    await user.click(screen.getByRole("button", { name: "Confirm" }));

    expect(
      await screen.findByText("That code is not right. Check it and try again."),
    ).toBeVisible();
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("the right code changes the address and re-reads the account", async () => {
    const user = userEvent.setup();
    const onChanged = vi.fn();
    signInAs("merchant@example.com");
    renderSection(onChanged);

    await user.click(screen.getByRole("button", { name: "Change" }));
    await user.type(screen.getByLabelText("New email address"), "new@example.com");
    await user.click(screen.getByRole("button", { name: "Email a code" }));

    const code = await screen.findByLabelText("Verification code");
    await user.type(code, "123456");
    await user.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
    // The row is back to its closed form: the flow's inputs are gone, and the
    // address shown follows once the page re-reads the account.
    expect(screen.queryByLabelText("Verification code")).toBeNull();
    expect(screen.getByRole("button", { name: "Change" })).toBeVisible();
  });

  it("an invalid address never reaches the code screen", async () => {
    const user = userEvent.setup();
    const onChanged = vi.fn();
    signInAs("merchant@example.com");
    renderSection(onChanged);

    await user.click(screen.getByRole("button", { name: "Change" }));
    // The button stays disabled for a malformed address, so nothing is sent.
    await user.type(screen.getByLabelText("New email address"), "not-an-address");
    expect(screen.getByRole("button", { name: "Email a code" }).hasAttribute("disabled")).toBe(
      true,
    );
    expect(onChanged).not.toHaveBeenCalled();
  });
});
