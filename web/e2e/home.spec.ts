import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

/**
 * The dashboard against e2e/stub-api.mjs.
 *
 * Gum is API-first, so the dashboard is the account's control panel and
 * nothing else: the API key, the balance of settled deposits and the flow
 * that withdraws it — with the sign-in email beside it, changeable — and the
 * deposits themselves, read-only. Deposit requests are created through the
 * API, which these specs exercise directly against the stub.
 */

const OTP = "123456";
const STUB = "http://127.0.0.1:4010";

async function signIn(page: Page, email: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "Get Started" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(email);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
}

/**
 * Creates a deposit request through the API, the way a merchant's
 * application does — the dashboard has no form for it any more. The stub
 * shares deposits across accounts, so the dashboard sees whatever this
 * creates.
 */
async function createThroughApi(
  request: APIRequestContext,
  body: Record<string, unknown>,
) {
  const response = await request.post(`${STUB}/v1/deposit-requests`, {
    headers: {
      authorization: "Bearer stub-dashboard-token",
      "idempotency-key": crypto.randomUUID(),
    },
    data: body,
  });
  expect(response.status()).toBe(201);
  return (await response.json()) as { id: string; heading: string | null };
}

test("the dashboard is the control panel: key, balance, deposits — and nothing to issue with", async ({
  page,
}) => {
  await signIn(page, "panel@example.com");

  await expect(page.getByRole("region", { name: "API key" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Balance." })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Deposits." })).toBeVisible();

  // The removed flows are gone, root and branch: nothing issues a deposit
  // request, and nothing manages customers or issuer identities.
  await expect(page.getByRole("button", { name: "New deposit request" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Customers." })).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Issuer identities" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "New customer" })).toHaveCount(0);
  await expect(page).not.toHaveURL(/\/dashboard\/(customers|new)/);
});

test("a request created through the API is shown read-only, with its issuer id verbatim", async ({
  page,
  request,
}) => {
  await signIn(page, "readonly@example.com");

  const created = await createThroughApi(request, {
    amount: "12.34",
    payout_address: "0x1111111111111111111111111111111111111111",
    issuer: { name: "Acme EU", email: "billing@acme.example" },
    issuer_id: "acme-eu-2026",
    payer: { name: "Test Co" },
    heading: "API-created request",
    verification: { email: { expected_email: "ap@acme.example" }, wallet_attestation: false },
  });

  // The table loaded before the request existed; a reload is what a merchant
  // with the API open in another window would see arrive.
  await page.reload();
  const row = page.getByRole("row").filter({ hasText: created.heading ?? "" });
  await expect(row).toBeVisible();
  await expect(row).toContainText("acme-eu-2026");
  // Reading is all it can do: the row opens its detail, and nothing else.
  await row.click();
  await expect(page.getByRole("progressbar", { name: "Received" })).toBeVisible();
  await expect(page.getByText("acme-eu-2026").first()).toBeVisible();
});

test("the customer filter narrows the query to one customer's deposits", async ({ page }) => {
  await signIn(page, "filter@example.com");

  await page.getByLabel("Customer").click();
  await page.getByRole("option", { name: "Globex Corporation" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Consulting — August" })).toHaveCount(1);
  await expect(page.getByRole("row").filter({ hasText: "Retainer — September" })).toHaveCount(0);

  await page.getByLabel("Customer").click();
  await page.getByRole("option", { name: "Any customer" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Retainer — September" })).toHaveCount(1);
});

test("the sign-in email can be changed, and the account keeps everything it had", async ({
  page,
}) => {
  await signIn(page, "rename@example.com");

  const section = page.getByRole("region", { name: "Account" });
  await expect(section).toContainText("rename@example.com");

  // The change is a confirmation, not a typing contest: a code goes to the
  // new address, and the address moves only once it is confirmed.
  await section.getByRole("button", { name: "Change" }).click();
  await section.getByLabel("New email address").fill("renamed@example.com");
  await section.getByRole("button", { name: "Email a code" }).click();
  await expect(section.getByText("renamed@example.com")).toBeVisible();

  // A wrong code is said so, in place.
  await section.getByLabel("Verification code").fill("000000");
  await section.getByRole("button", { name: "Confirm" }).click();
  await expect(section.getByRole("alert")).toBeVisible();

  await section.getByLabel("Verification code").fill(OTP);
  await section.getByRole("button", { name: "Confirm" }).click();
  await expect(section).toContainText("renamed@example.com");
  await expect(section.getByText("rename@example.com")).toHaveCount(0);

  // The same merchant is still signed in: same wallet, same loaded data —
  // the page did not empty and re-fetch around them.
  const wallet = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("gum.privy-stub.session") ?? "{}").wallet,
  );
  expect(wallet).toMatch(/^0x[0-9a-f]{40}$/);
  await expect(section).toContainText(wallet);
  await expect(page.getByRole("region", { name: "API key" })).toBeVisible();
});

/**
 * A page load is a burst of reads from one account, and the API meters each
 * account. Congestion must never reach the page: the client retries after
 * `Retry-After`, and the resource cache retries again behind the skeleton.
 */
test("a rate-limited page load settles quietly, with one request per resource", async ({
  page,
}) => {
  const seen = new Map<string, number>();
  let refused = 0;
  await page.route(
    (url) => url.pathname.startsWith("/v1/"),
    async (route) => {
      const request = route.request();
      // Only the dashboard's own reads: the sign-in dialog polls the account
      // while the wallet is created, and that is not the page load.
      if (request.method() !== "GET" || !page.url().includes("/dashboard")) {
        return route.continue();
      }
      const path = new URL(request.url()).pathname + new URL(request.url()).search;
      seen.set(path, (seen.get(path) ?? 0) + 1);
      // Each resource's first read is refused, up to three of them, as an
      // overrun bucket would refuse the burst.
      if (seen.get(path) === 1 && refused < 3) {
        refused += 1;
        await route.fulfill({
          status: 429,
          contentType: "application/json",
          headers: { "Retry-After": "1" },
          body: JSON.stringify({
            error: { code: "rate_limited", message: "Per-account request limit exceeded" },
            request_id: "01a09f98-7a10-7551-bb0c-61a13ec56e1d",
          }),
        });
        return;
      }
      await route.continue();
    },
  );
  await signIn(page, "metered@example.com");

  await expect(page.getByRole("region", { name: "API key" })).toBeVisible();
  await expect(page.getByText(/Couldn't load/)).toHaveCount(0);
  await expect(page.getByText(/request limit exceeded/)).toHaveCount(0);
  await expect(page.getByText(/01a09f98/)).toHaveCount(0);

  // Deduplicated: no resource was read twice silently, and each refused read
  // was sent again exactly once.
  expect(refused).toBe(3);
  expect(seen.size).toBeGreaterThanOrEqual(2);
  for (const [path, count] of seen) {
    expect(count, path).toBeLessThanOrEqual(2);
  }
  // Every refused read that stayed wanted was sent again, and no resource was
  // read twice silently: the burst never doubled, and it settled.
  const total = [...seen.values()].reduce((sum, count) => sum + count, 0);
  expect(total).toBeGreaterThanOrEqual(seen.size);
  expect(total).toBeLessThanOrEqual(seen.size + refused);
});

test("a read that keeps failing becomes a page that says so, and recovers on retry", async ({
  page,
}) => {
  let broken = true;
  await page.route(
    (url) => url.pathname === "/v1/account",
    async (route) => {
      // The sign-in dialog reads the account too, while the wallet is made;
      // the outage is the dashboard's to handle.
      if (route.request().method() !== "GET" || !broken || !page.url().includes("/dashboard")) {
        return route.continue();
      }
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({
          error: { code: "internal_error", message: "upstream timeout" },
          request_id: "01a09f98-0000-7551-bb0c-61a13ec56e1d",
        }),
      });
    },
  );
  await signIn(page, "outage@example.com");

  // The skeleton holds through the retries; only then does the page say so,
  // in its own words, with the API's message kept to the small print.
  const problem = page.getByRole("alert").filter({ hasText: "Couldn't load your dashboard." });
  await expect(problem).toBeVisible({ timeout: 30_000 });
  await expect(problem).toContainText("Couldn't load your dashboard.");
  await expect(problem).toContainText("Gum is temporarily unavailable.");
  await expect(problem).toContainText(
    "upstream timeout (request 01a09f98-0000-7551-bb0c-61a13ec56e1d)",
  );

  broken = false;
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("region", { name: "API key" })).toBeVisible();
  await expect(page.getByText(/Couldn't load/)).toHaveCount(0);
});
