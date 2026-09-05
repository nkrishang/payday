import { expect, test } from "@playwright/test";

/**
 * Signing up from the landing page, against e2e/stub-api.mjs playing the OTP
 * issuer. There is no separate registration: the API provisions an account the
 * first time it sees a verified identity, so this is the same exchange the
 * dashboard's own login page runs, reached from the hero instead.
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
  // Another code is not on offer while the first one still works.
  await expect(dialog.getByRole("button", { name: /Resend in \d:\d\d/ })).toBeDisabled();

  // Nothing to submit until the code is whole.
  await expect(dialog.getByRole("button", { name: "Continue" })).toBeDisabled();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();

  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole("heading", { name: "Deposit requests." })).toBeVisible();

  // The token is the session: this tab only, never localStorage.
  const storage = await page.evaluate(() => ({
    session: sessionStorage.getItem("payday.dashboard.session"),
    local: localStorage.length,
  }));
  expect(storage.session).toContain("stub-dashboard-token");
  expect(storage.local).toBe(0);
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

test("a new code is offered only once the one in flight has expired", async ({ page }) => {
  // The window is five minutes of real time; the page is given a clock it can
  // be moved through instead.
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(EMAIL);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await expect(dialog.getByRole("button", { name: "Resend in 5:00" })).toBeDisabled();

  await page.clock.fastForward("04:00");
  await expect(dialog.getByRole("button", { name: "Resend in 1:00" })).toBeDisabled();

  await page.clock.fastForward("01:00");
  const resend = dialog.getByRole("button", { name: "Resend code" });
  await expect(resend).toBeEnabled();
  await expect(dialog.getByText(`The code we sent to ${EMAIL} has expired`)).toBeVisible();

  // Resending restarts the window rather than leaving two codes alive.
  await resend.click();
  await expect(dialog.getByRole("button", { name: "Resend in 5:00" })).toBeDisabled();
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
  await page.evaluate(() => {
    sessionStorage.setItem(
      "payday.dashboard.session",
      JSON.stringify({ accessToken: "stub-dashboard-token", expiresAt: Date.now() + 300_000 }),
    );
  });

  await page.getByRole("button", { name: "Start Building" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
