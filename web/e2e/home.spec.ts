import { expect, test, type Page } from "@playwright/test";

/**
 * The dashboard against e2e/stub-api.mjs.
 *
 * The stub keeps issuer identities per mailbox, exactly as the API keeps them
 * per account, so each test signs in as its own merchant: one that has set
 * nothing up, and one that has. Deposit requests and customers stay shared, which the
 * rest of the suite depends on.
 */

const OTP = "123456";
const SAVED_WALLET = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

async function signIn(page: Page, email: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(email);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
}

/** The account's own wallet, as the Privy stub minted it at sign-in. */
async function accountWallet(page: Page): Promise<string> {
  return page.evaluate(
    () => JSON.parse(sessionStorage.getItem("payday.privy-stub.session") ?? "{}").wallet,
  );
}

function truncate(address: string): string {
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
}

/** Answers the deposit requests probe as an empty account, so setup is the whole page. */
async function withoutHistory(page: Page) {
  await page.route(
    (url) => url.pathname === "/v1/deposit-requests",
    async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ deposit_requests: [], next_cursor: null }),
      });
    },
  );
}

/**
 * The identity every request is issued under. Reached from the dashboard,
 * because an account that has issued before still sees its requests first —
 * and it ends in the composer, since a proven mailbox is all an identity
 * needs: deposits settle to the account's own wallet.
 */
async function setUpIdentity(page: Page, name: string) {
  await page.getByRole("button", { name: "New deposit request" }).click();
  await page.getByLabel("Issued by").fill(name);
  await page.getByLabel("Contact address").fill("billing@acme.example");
  await page.getByRole("button", { name: "Send code" }).click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
}

test("a new merchant is put straight to work: identity, contact, first request", async ({
  page,
}) => {
  await withoutHistory(page);
  await signIn(page, "firstrun@example.com");
  const wallet = await accountWallet(page);

  // The whole form is on the page: no section is hidden behind the one before,
  // and there is no wallet to type — the account already has one.
  const identity = page.getByRole("region", { name: "Identity" });
  const verify = page.getByRole("region", { name: "Verify email" });
  await expect(identity.getByLabel("Issued by")).toBeEnabled();
  await expect(verify.getByLabel("One-time code")).toBeDisabled();
  await expect(page.getByLabel("Payout address")).toHaveCount(0);

  // Nothing advances until the section is answerable, and a field says why as
  // soon as it holds something that will not do.
  const send = page.getByRole("button", { name: "Send code" });
  await expect(send).toBeDisabled();
  await identity.getByLabel("Issued by").fill("Acme Inc.");
  await identity.getByLabel("Contact address").fill("not-an-email");
  await expect(identity.getByText("Not a valid email address.")).toBeVisible();
  await expect(send).toBeDisabled();
  await identity.getByLabel("Contact address").fill("billing@acme.example");
  await expect(send).toBeEnabled();
  await send.click();

  // The contact address is confirmed, not merely typed.
  await expect(verify.getByText("Sent to billing@acme.example.")).toBeVisible();
  await verify.getByLabel("One-time code").fill("000000");
  await page.getByRole("button", { name: "Confirm code" }).click();
  await expect(page.getByText(/code is not valid/)).toBeVisible();
  await verify.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();

  // Setting up leads into a guided tour, not the blank composer — and there
  // is no way to skip it.
  await expect(page.getByRole("heading", { name: "Welcome to Payday." })).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel" })).toHaveCount(0);

  // Every field is fixed and inert: this is Payday billing itself, so the
  // merchant can watch the whole product work before using it for real. It
  // settles to the account's own wallet.
  await expect(page.getByLabel("Amount")).toHaveValue("0.000001");
  await expect(page.getByText(truncate(wallet))).toBeVisible();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByLabel("Payer")).toHaveValue("Payday");
  await expect(page.getByLabel("Email")).toHaveValue("onboarding@payday.sh");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByLabel("Expected payer email")).toHaveValue("onboarding@payday.sh");
  await page.getByRole("button", { name: "Continue" }).click();

  // The review is real values, exactly as the real composer's is.
  const summary = page.getByLabel("Request summary");
  await expect(summary).toContainText("0.000001");
  await expect(summary).toContainText("Acme Inc.");
  await expect(summary).toContainText("Payday");
  await expect(summary).toContainText("Verified email");
  await expect(summary).toContainText(truncate(wallet));

  // A payer typed fresh, exactly like the real composer: Payday
  // becomes a saved customer, not just a name on this one invoice.
  const customerCreated = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith("/v1/customers"),
  );
  const created = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith("/v1/deposit-requests"),
  );
  await page.getByRole("button", { name: "Issue deposit request" }).click();
  expect((await customerCreated).postDataJSON()).toEqual({
    name: "Payday",
    email: "onboarding@payday.sh",
  });
  const body = (await created).postDataJSON();
  expect(body.customer_id).toBeTruthy();
  expect(body.payout_address).toBe(wallet);
  expect(body.payer).toMatchObject({ name: "Payday", email: "onboarding@payday.sh" });
  expect(body.payer_policy).toEqual({
    mode: "verified_email",
    expected_email: "onboarding@payday.sh",
  });

  // The success screen is not the usual three-CTA one — it shows the real
  // payer's view instead, live, and only lets the merchant through once it
  // has actually settled.
  await expect(page.getByRole("heading", { name: "Deposit request issued." })).toBeVisible();
  await expect(page.getByRole("link", { name: "Open the payer's view" })).toHaveCount(0);
  // The stub settles synchronously, so "disabled" is not reliably observable
  // here the way it is against the real, slower chain — only that it does not
  // let the merchant through until settlement has actually been reported.
  const getStarted = page.getByRole("button", { name: "Get started" });
  await expect(page.getByText("Transaction")).toBeVisible();
  await expect(getStarted).toBeEnabled();
  await getStarted.click();

  // Back on a dashboard whose list is still empty (the probe is stubbed): the
  // table is there with its columns and its own control, as the others are.
  const requests = page.getByRole("region", { name: "Deposits" });
  await expect(requests.getByRole("heading", { name: "Deposits." })).toBeVisible();
  await expect(requests.getByRole("columnheader", { name: "Request" })).toBeVisible();
  await expect(requests.getByText("No deposit requests yet.")).toBeVisible();
  await expect(requests.getByRole("button", { name: "New deposit request" })).toHaveCount(1);

  // Payday itself is there in the customers table, not hidden: a real,
  // reusable counterparty like any other. Scoped to the customer row's own
  // link (by href) since the wordmark in the header is also named "Payday".
  await expect(page.locator('a[href*="/dashboard/customers/"]', { hasText: "Payday" })).toBeVisible();
});

test("the composer keeps a running preview and can be stepped back through", async ({ page }) => {
  await signIn(page, "preview@example.com");
  await setUpIdentity(page, "Acme Inc.");
  const wallet = await accountWallet(page);

  const preview = page.getByRole("complementary");
  await expect(preview).toContainText("0.00");
  // The identity and the account's wallet are already on the request, unasked.
  await expect(preview).toContainText("Acme Inc.");
  await expect(preview).toContainText(truncate(wallet));
  // One destination means no question about it.
  await expect(page.getByRole("radiogroup", { name: "Settles to" })).toHaveCount(0);

  await page.getByLabel("Amount").fill("40.5");
  await expect(preview).toContainText("40.50");
  // Verification is unset until a policy is chosen, and reads as unset.
  await expect(
    preview.locator("dt", { hasText: "Verification" }).locator("xpath=following-sibling::dd"),
  ).toHaveText("—");

  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Payer").fill("Globex LLC");
  await page.getByLabel("Reason").fill("Consulting");
  await expect(preview).toContainText("Globex LLC");

  // Back keeps what was typed rather than starting the request over.
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByLabel("Amount")).toHaveValue("40.5");

  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("heading", { name: "Deposits." })).toBeVisible();
});

test("a set-up merchant sees their requests, identities, and customers on one page", async ({
  page,
}) => {
  await signIn(page, "history@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  await expect(page.getByRole("heading", { name: "Deposits." })).toBeVisible();
  await expect(page.getByRole("button", { name: /Consulting — August/ })).toBeVisible();

  // The identity, with its proof, is managed here too. A line each: the name,
  // whether it can be issued under, and where it settles — the account's own
  // wallet, with no saved wallet of its own.
  const identities = page.getByRole("region", { name: "Issuer identities" });
  const row = identities.getByRole("button", { expanded: false });
  await expect(row).toContainText("Acme Inc.");
  await expect(row).toContainText("Verified");
  await expect(row).toContainText("Payday wallet");
  await row.click();
  // Opening a row opens its form: the fields are there, not behind an Edit.
  await expect(identities.getByText(/Settles to your Payday wallet/)).toBeVisible();
  await expect(identities.getByLabel("Issued by")).toHaveValue("Acme Inc.");
  await expect(identities.getByRole("button", { name: "Save", exact: true })).toBeDisabled();

  await expect(page.getByRole("heading", { name: "Customers." })).toBeVisible();
  await expect(page.getByRole("link", { name: "Globex Corporation" })).toBeVisible();

  await page.getByRole("button", { name: "New deposit request" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
});

test("a payer becomes a customer, and the next request can pick them", async ({ page }) => {
  await signIn(page, "repeat@example.com");
  await setUpIdentity(page, "Acme Inc.");

  await page.getByLabel("Amount").fill("80");
  await page.getByRole("button", { name: "Continue" }).click();
  const billed = `Initech ${Date.now()}`;
  await page.getByLabel("Payer").fill(billed);
  await page.getByLabel("Email").fill("ap@initech.example");
  await page.getByLabel("Reason").fill("September invoice");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Issue deposit request" }).click();
  await expect(page.getByRole("heading", { name: "Deposit request issued." })).toBeVisible();

  // It is in the customers table without anyone saving it by hand.
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("link", { name: billed })).toBeVisible();

  // And the next request offers them rather than asking for them again.
  await page.getByRole("button", { name: "New deposit request" }).click();
  await page.getByLabel("Amount").fill("20");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Customer").click();
  await page.getByRole("option", { name: billed }).click();
  await expect(page.getByLabel("Payer")).toHaveValue(billed);
  await expect(page.getByLabel("Email")).toHaveValue("ap@initech.example");
});

test("a saved wallet can be added, chosen on a request, and removed again", async ({ page }) => {
  await signIn(page, "wallets@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();
  const wallet = await accountWallet(page);

  const identities = page.getByRole("region", { name: "Issuer identities" });
  const row = identities.getByRole("button", { expanded: false });
  await expect(row).toContainText("Payday wallet");
  await row.click();

  await identities.getByRole("button", { name: "Add wallet" }).click();
  await identities.getByLabel("Payout address").fill(SAVED_WALLET);
  await identities.getByLabel("Label").fill("Treasury");
  await identities.getByRole("button", { name: "Add", exact: true }).click();

  const save = identities.getByRole("button", { name: "Save", exact: true });
  await expect(save).toBeEnabled();
  await save.click();

  // Wait for the write to land before judging the button, so this cannot pass
  // on the moment it is disabled merely because a request is in flight.
  await expect(identities.getByRole("button", { expanded: true })).toContainText("1 saved wallet");
  await expect(identities.getByText("Treasury")).toBeVisible();
  // The API's own answer became the new baseline: nothing is left unsaved.
  await expect(save).toBeDisabled();

  // With two destinations the composer asks, defaulting to the account's own.
  await page.getByRole("button", { name: "New deposit request" }).click();
  const settles = page.getByRole("radiogroup", { name: "Settles to" });
  await expect(settles.getByRole("radio", { name: "Payday wallet" })).toBeChecked();
  const preview = page.getByRole("complementary");
  await expect(preview).toContainText(truncate(wallet));
  await settles.getByRole("radio", { name: "Treasury" }).click();
  await expect(preview).toContainText(truncate(SAVED_WALLET));
  await page.getByRole("button", { name: "Cancel" }).click();

  // Dropping the saved wallet leaves the identity on the account's own again.
  await identities.getByRole("button", { expanded: false }).click();
  await identities.getByRole("button", { name: "Remove Treasury" }).click();
  await expect(save).toBeEnabled();
  await save.click();
  await expect(identities.getByRole("button", { expanded: true })).toContainText("Payday wallet");
  await expect(identities.getByText("Treasury")).toBeHidden();
});

test("two identities cannot share a name", async ({ page }) => {
  await signIn(page, "names@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  const identities = page.getByRole("region", { name: "Issuer identities" });
  await identities.getByRole("button", { name: "New issuer identity" }).click();

  // However it is cased, and before the request is ever made.
  const setup = page.getByRole("region", { name: "Identity" });
  await expect(setup.getByLabel("Issued by")).toBeEnabled();
  const send = page.getByRole("button", { name: "Send code" });
  await setup.getByLabel("Issued by").fill("acme inc.");
  await setup.getByLabel("Contact address").fill("second@acme.example");
  await expect(setup.getByText("Another identity already uses this name.")).toBeVisible();
  await expect(send).toBeDisabled();

  // A free name goes through.
  await setup.getByLabel("Issued by").fill("Acme EU");
  await expect(send).toBeEnabled();
  await send.click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  // And a rename onto the other identity's name is refused the same way.
  const second = identities.getByRole("button", { expanded: false }).first();
  await expect(second).toContainText("Acme EU");
  await second.click();
  const row = identities.getByRole("button", { expanded: true }).locator("..");
  await row.getByLabel("Issued by").fill("acme inc.");
  await expect(row.getByText("Another identity already uses this name.")).toBeVisible();
  await expect(row.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
});

test("a request row opens in place, showing an abbreviated link to the payer's view", async ({
  page,
}) => {
  await signIn(page, "detail@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("row").filter({ hasText: "Consulting — August" }).click();
  // In place: the list stays where it was, filters and page included.
  await expect(page).toHaveURL(/\/dashboard$/);

  // Abbreviated to the link itself rather than a full preview of the page.
  const payerLink = page.getByRole("link", { name: /\/pay\// });
  await expect(payerLink).toBeVisible();
  await expect(payerLink).toHaveAttribute("href", /\/pay\//);
  await expect(payerLink).toHaveAttribute("target", "_blank");
});

test("the payer's view link opens unlocked for the issuing merchant, without verifying it", async ({
  page,
}) => {
  await signIn(page, "preview-merchant@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  // A freshly issued, still-unverified verified_email request — no ambiguity
  // with a settled request's own receipt access.
  await page.getByRole("button", { name: "New deposit request" }).click();
  await page.getByLabel("Amount").fill("12");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Payer").fill("Globex LLC");
  await page.getByLabel("Reason").fill("Consulting");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("radio", { name: "Verified email" }).check();
  await page.getByLabel("Expected payer email").fill("payer@example.com");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Issue deposit request" }).click();
  await expect(page.getByRole("heading", { name: "Deposit request issued." })).toBeVisible();

  // The preview session is minted up front, so the link only becomes the
  // enhanced one once that lands — a bare "/pay/" href would still open,
  // just locked. Opened as a fresh navigation (rather than through the
  // link's own click and popup) so the assertion is about what the URL
  // itself carries, not about this browser's popup-handling.
  const payerLink = page.getByRole("link", { name: "Open the payer's view" });
  await expect(payerLink).toHaveAttribute("href", /#ps=/);
  const href = (await payerLink.getAttribute("href"))!;
  const browser = page.context().browser()!;

  const merchant = await (await browser.newContext()).newPage();
  await merchant.goto(href);
  // Everything a locked link would withhold is visible, from a click alone —
  // no code, no form.
  await expect(merchant.getByRole("region", { name: "Deposit request", exact: true })).toBeVisible();
  await expect(merchant.getByText("Verification required")).toHaveCount(0);
  await merchant.context().close();

  // Previewing is not verifying: a stranger holding the bare link (without
  // the merchant's own fragment) is still locked, exactly as before the
  // merchant looked at it.
  const bareUrl = href.split("#")[0]!;
  const stranger = await (await browser.newContext()).newPage();
  await stranger.goto(bareUrl);
  await expect(stranger.getByText("Verification required")).toBeVisible();
  await stranger.context().close();
});

test("an identity can be renamed, and moving its contact address unproves it", async ({ page }) => {
  await signIn(page, "manage@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  // A row is a line until it is opened; the details and the actions are inside.
  const identities = page.getByRole("region", { name: "Issuer identities" });
  await identities.getByRole("button", { expanded: false }).click();

  // Save lights up only once something actually differs from what is stored.
  const save = identities.getByRole("button", { name: "Save", exact: true });
  await expect(save).toBeDisabled();
  await identities.getByLabel("Contact address").fill("support@acme.example");
  await expect(save).toBeEnabled();
  await identities.getByLabel("Contact address").fill("billing@acme.example");
  await expect(save).toBeDisabled();
  await identities.getByLabel("Contact address").fill("support@acme.example");
  await save.click();

  // A different mailbox is a different claim, so the proof does not carry.
  const opened = identities.getByRole("button", { expanded: true });
  await expect(opened).toContainText("Unverified");
  await expect(opened).toContainText("support@acme.example");

  // An unproven identity cannot be issued under, and the flow resumes at the
  // step that is unfinished rather than starting over.
  await page.getByRole("button", { name: "New deposit request" }).click();
  await expect(
    page.getByRole("region", { name: "Verify email" }).getByText("Sent to support@acme.example."),
  ).toBeVisible();
});
