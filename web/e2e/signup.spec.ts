import { expect, test } from "@playwright/test";

/**
 * Signing up from the landing page, with `test/privy-stub.tsx` standing in
 * for Privy. There is no separate registration: the API provisions an account
 * the first time it sees a verified identity, so this is the same exchange the
 * dashboard runs on, reached from the hero.
 */

const OTP = "123456";
const EMAIL = "founder@acme.test";

test("the hero opens a sign-up dialog that signs a new merchant in", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();

  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Start building." })).toBeVisible();
  // The mailbox is focused, not the close button.
  await expect(dialog.getByLabel("Email")).toBeFocused();

  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();

  await expect(dialog.getByRole("heading", { name: "Check your email." })).toBeVisible();
  await expect(dialog.getByText(`We sent a six-digit code to ${EMAIL}`)).toBeVisible();
  // Another code is not on offer straight away.
  await expect(dialog.getByRole("button", { name: /Resend in \d:\d\d/ })).toBeDisabled();

  // Nothing to submit until the code is whole.
  await expect(dialog.getByRole("button", { name: "Continue" })).toBeDisabled();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();

  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole("heading", { name: "Deposits." })).toBeVisible();

  // The session is Privy's to keep (the stub keeps it in this tab); the page
  // itself stores no credential of its own anywhere.
  const storage = await page.evaluate(() => ({
    stub: sessionStorage.getItem("payday.privy-stub.session"),
    keys: [...Object.keys(sessionStorage), ...Object.keys(localStorage)],
  }));
  expect(storage.stub).toContain("stub-dashboard-token");
  expect(storage.keys.filter((key) => key.startsWith("payday.") && !key.includes("privy"))).toEqual(
    [],
  );
});

test("a wrong code is refused without losing the dialog, and the digits stay selected", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();

  await dialog.getByLabel("One-time code").fill("000000");
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(dialog.getByText(/code is not valid/)).toBeVisible();
  await expect(page).toHaveURL("/");

  // Typing over the rejected code replaces it rather than appending to it.
  await page.keyboard.type(OTP);
  await expect(dialog.getByLabel("One-time code")).toHaveValue(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
});

test("a new code is offered only once the cooldown has run", async ({ page }) => {
  // The window is a minute of real time; the page is given a clock it can be
  // moved through instead.
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await expect(dialog.getByRole("button", { name: "Resend in 1:00" })).toBeDisabled();

  await page.clock.fastForward("00:30");
  await expect(dialog.getByRole("button", { name: "Resend in 0:30" })).toBeDisabled();

  await page.clock.fastForward("00:30");
  const resend = dialog.getByRole("button", { name: "Resend code" });
  await expect(resend).toBeEnabled();

  // Resending restarts the window and clears whatever was half-typed.
  await dialog.getByLabel("One-time code").fill("12");
  await resend.click();
  await expect(dialog.getByRole("button", { name: "Resend in 1:00" })).toBeDisabled();
  await expect(dialog.getByText(`We sent a six-digit code to ${EMAIL}`)).toBeVisible();
  await expect(dialog.getByLabel("One-time code")).toHaveValue("");

  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
});

test("closing the dialog abandons the attempt", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await expect(dialog.getByLabel("One-time code")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();

  // Reopening starts at the mailbox again, not on a code that has gone stale.
  await page.getByRole("button", { name: "Start Building" }).click();
  await expect(dialog.getByRole("heading", { name: "Start building." })).toBeVisible();
  await expect(dialog.getByLabel("One-time code")).toBeHidden();
});

test("a visitor who still holds a session goes straight to the dashboard", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);

  // Back on the landing page, "Start Building" has nothing to ask.
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
