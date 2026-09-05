import { expect, test, type Page } from "@playwright/test";

/**
 * The dashboard against e2e/stub-api.mjs.
 *
 * The stub keeps issuer identities per mailbox, exactly as the API keeps them
 * per account, so each test signs in as its own merchant: one that has set
 * nothing up, and one that has. Payments and customers stay shared, which the
 * rest of the suite depends on.
 */

const OTP = "123456";
const PAYOUT = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const SECOND_PAYOUT = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

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

/** Answers the payments probe as an empty account, so setup is the whole page. */
async function withoutHistory(page: Page) {
  await page.route(
    (url) => url.pathname === "/v1/payments",
    async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ payments: [], next_cursor: null }),
      });
    },
  );
}

/**
 * The identity every request is issued under. Reached from the dashboard,
 * because an account that has issued before still sees its requests first.
 */
async function setUpIdentity(page: Page, name: string) {
  await page.getByRole("button", { name: "New deposit request" }).click();
  await page.getByLabel("Issued by").fill(name);
  await page.getByLabel("Contact address").fill("billing@acme.example");
  await page.getByRole("button", { name: "Send code" }).click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await page.getByLabel("Payout address").fill(PAYOUT);
  await page.getByLabel("Label").fill("Treasury");
  await page.getByRole("button", { name: "Save identity" }).click();
}

test("a new merchant is put straight to work: identity, contact, wallet, first request", async ({
  page,
}) => {
  await withoutHistory(page);
  await signIn(page, "firstrun@example.com");

  // The whole form is on the page: no section is hidden behind the one before.
  const identity = page.getByRole("region", { name: "Identity" });
  const verify = page.getByRole("region", { name: "Verify email" });
  const wallet = page.getByRole("region", { name: "Wallet" });
  await expect(identity.getByLabel("Issued by")).toBeEnabled();
  await expect(verify.getByLabel("One-time code")).toBeDisabled();
  await expect(wallet.getByLabel("Payout address")).toBeDisabled();

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

  const save = page.getByRole("button", { name: "Save identity" });
  await expect(save).toBeDisabled();
  await wallet.getByLabel("Payout address").fill("0xnope");
  await expect(wallet.getByText(/Not a valid address/)).toBeVisible();
  await expect(save).toBeDisabled();
  await wallet.getByLabel("Payout address").fill(PAYOUT);

  // A label is a short handle, not free text.
  await wallet.getByLabel("Label").fill("-nope;");
  await expect(wallet.getByText("Not a valid label.")).toBeVisible();
  await expect(save).toBeDisabled();
  await wallet.getByLabel("Label").fill("Treasury");
  await expect(save).toBeEnabled();
  await save.click();

  // Setting up leads into a guided tour, not the blank composer — and there
  // is no way to skip it.
  await expect(page.getByRole("heading", { name: "Welcome to Payday." })).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel" })).toHaveCount(0);

  // Every field is fixed and inert: this is Payday billing itself, so the
  // merchant can watch the whole product work before using it for real.
  await expect(page.getByLabel("Amount")).toHaveValue("0.000001");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByLabel("Billed to")).toHaveValue("Payday");
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

  // Billed to a party typed fresh, exactly like the real composer: Payday
  // becomes a saved customer, not just a name on this one invoice.
  const customerCreated = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith("/v1/customers"),
  );
  const created = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith("/v1/payments"),
  );
  await page.getByRole("button", { name: "Issue deposit request" }).click();
  expect((await customerCreated).postDataJSON()).toEqual({
    name: "Payday",
    email: "onboarding@payday.sh",
  });
  const body = (await created).postDataJSON();
  expect(body.customer_id).toBeTruthy();
  expect(body.bill_to).toMatchObject({ name: "Payday", email: "onboarding@payday.sh" });
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
  const requests = page.getByRole("region", { name: "Deposit requests" });
  await expect(requests.getByRole("heading", { name: "Deposit requests." })).toBeVisible();
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

  const preview = page.getByRole("complementary");
  await expect(preview).toContainText("0.00");
  // The identity and its wallet are already on the request, unasked.
  await expect(preview).toContainText("Acme Inc.");
  await expect(preview).toContainText("0x7099…79C8");

  await page.getByLabel("Amount").fill("40.5");
  await expect(preview).toContainText("40.50");
  // Verification is unset until a policy is chosen, and reads as unset.
  await expect(
    preview.locator("dt", { hasText: "Verification" }).locator("xpath=following-sibling::dd"),
  ).toHaveText("—");

  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Billed to").fill("Globex LLC");
  await expect(preview).toContainText("Globex LLC");

  // Back keeps what was typed rather than starting the request over.
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByLabel("Amount")).toHaveValue("40.5");

  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("heading", { name: "Deposit requests." })).toBeVisible();
});

test("a set-up merchant sees their requests, identities, and customers on one page", async ({
  page,
}) => {
  await signIn(page, "history@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  await expect(page.getByRole("heading", { name: "Deposit requests." })).toBeVisible();
  await expect(page.getByRole("button", { name: /Consulting — August/ })).toBeVisible();

  // The identity, with its proof and its wallet, is managed here too.
  // A line each: the name, whether it can be issued under, and how many
  // wallets. The wallets themselves are a detail of the opened row.
  const identities = page.getByRole("region", { name: "Issuer identities" });
  const row = identities.getByRole("button", { expanded: false });
  await expect(row).toContainText("Acme Inc.");
  await expect(row).toContainText("Verified");
  await expect(row).toContainText("1 wallet");
  await row.click();
  // Opening a row opens its form: the fields are there, not behind an Edit.
  await expect(identities.getByText("Treasury")).toBeVisible();
  await expect(identities.getByLabel("Issued by")).toHaveValue("Acme Inc.");
  await expect(identities.getByRole("button", { name: "Save" })).toBeDisabled();

  await expect(page.getByRole("heading", { name: "Customers." })).toBeVisible();
  await expect(page.getByRole("link", { name: "Globex Corporation" })).toBeVisible();

  await page.getByRole("button", { name: "New deposit request" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
});

test("a billed party becomes a customer, and the next request can pick them", async ({ page }) => {
  await signIn(page, "repeat@example.com");
  await setUpIdentity(page, "Acme Inc.");

  await page.getByLabel("Amount").fill("80");
  await page.getByRole("button", { name: "Continue" }).click();
  const billed = `Initech ${Date.now()}`;
  await page.getByLabel("Billed to").fill(billed);
  await page.getByLabel("Email").fill("ap@initech.example");
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
  await expect(page.getByLabel("Billed to")).toHaveValue(billed);
  await expect(page.getByLabel("Email")).toHaveValue("ap@initech.example");
});

test("adding a wallet saves, and the row settles rather than staying dirty", async ({ page }) => {
  await signIn(page, "wallets@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  const identities = page.getByRole("region", { name: "Issuer identities" });
  const row = identities.getByRole("button", { expanded: false });
  await expect(row).toContainText("1 wallet");
  await row.click();

  await identities.getByRole("button", { name: "Add wallet" }).click();
  await identities.getByLabel("Payout address").fill(SECOND_PAYOUT);
  await identities.getByLabel("Label").fill("Secondary");
  await identities.getByRole("button", { name: "Add", exact: true }).click();

  const save = identities.getByRole("button", { name: "Save" });
  await expect(save).toBeEnabled();
  await save.click();

  // Wait for the write to land before judging the button, so this cannot pass
  // on the moment it is disabled merely because a request is in flight.
  await expect(identities.getByRole("button", { expanded: true })).toContainText("2 wallets");
  await expect(identities.getByText("Secondary")).toBeVisible();
  // The API's own answer became the new baseline: nothing is left unsaved.
  await expect(save).toBeDisabled();
});

test("removing the only wallet asks for its replacement instead of stranding the identity", async ({
  page,
}) => {
  await signIn(page, "lastwallet@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  const identities = page.getByRole("region", { name: "Issuer identities" });
  await identities.getByRole("button", { expanded: false }).click();

  // The only wallet is replaced, never simply removed.
  await identities.getByRole("button", { name: "Replace Treasury" }).click();
  await expect(identities.getByText(/replaced rather than removed/)).toBeVisible();
  const save = identities.getByRole("button", { name: "Save" });
  await expect(save).toBeDisabled();

  await identities.getByLabel("Payout address").fill(SECOND_PAYOUT);
  await identities.getByLabel("Label").fill("Replacement");
  await identities.getByRole("button", { name: "Add", exact: true }).click();
  await expect(save).toBeEnabled();
  await save.click();

  // One wallet still, the new one, and the dashboard never lost the identity.
  await expect(identities.getByRole("button", { expanded: true })).toContainText("1 wallet");
  await expect(identities.getByText("Replacement")).toBeVisible();
  await expect(identities.getByText("Treasury")).toBeHidden();
  await page.getByRole("button", { name: "New deposit request" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
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

  // A free name goes through, wallet and all.
  await setup.getByLabel("Issued by").fill("Acme EU");
  await expect(send).toBeEnabled();
  await send.click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await page.getByLabel("Payout address").fill(SECOND_PAYOUT);
  await page.getByRole("button", { name: "Save identity" }).click();
  await page.getByRole("button", { name: "Cancel" }).click();

  // And a rename onto the other identity's name is refused the same way.
  const second = identities.getByRole("button", { expanded: false }).first();
  await expect(second).toContainText("Acme EU");
  await second.click();
  const row = identities.getByRole("button", { expanded: true }).locator("..");
  await row.getByLabel("Issued by").fill("acme inc.");
  await expect(row.getByText("Another identity already uses this name.")).toBeVisible();
  await expect(row.getByRole("button", { name: "Save" })).toBeDisabled();
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

test("an identity can be renamed, and moving its contact address unproves it", async ({ page }) => {
  await signIn(page, "manage@example.com");
  await setUpIdentity(page, "Acme Inc.");
  await page.getByRole("button", { name: "Cancel" }).click();

  // A row is a line until it is opened; the details and the actions are inside.
  const identities = page.getByRole("region", { name: "Issuer identities" });
  await identities.getByRole("button", { expanded: false }).click();

  // Save lights up only once something actually differs from what is stored.
  const save = identities.getByRole("button", { name: "Save" });
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
