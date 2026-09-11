import { expect, test } from "@playwright/test";
import { API_PAGES, DOCS_PAGES } from "../components/docs/nav";

/**
 * The documentation is static and reads nothing from the API, so what the
 * suite checks is that it is all there and holds together: every page in
 * both tables of contents renders with its sidebar entry active, the
 * landing page's Docs link arrives here, the section tabs switch sidebars,
 * the reference sidebar carries a method badge per route, the outline and
 * the pager are wired, the sample tabs remember their choice, and nothing
 * scrolls sideways on a phone.
 */

test("the landing page's Read Docs link opens the documentation", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Read Docs" }).click();
  await expect(page).toHaveURL(/\/docs$/);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Stablecoin deposits");
});

test("every guide renders with its sidebar entry active", async ({ page }) => {
  for (const entry of DOCS_PAGES) {
    const response = await page.goto(entry.href);
    expect(response?.status(), entry.href).toBe(200);
    await expect(page.getByRole("heading", { level: 1 }), entry.href).toBeVisible();
    await expect(
      page.getByRole("navigation", { name: "Documentation" }).locator(`a[href="${entry.href}"]`),
      entry.href,
    ).toHaveAttribute("aria-current", "page");
  }
});

test("every reference page renders, with a method badge on each route", async ({ page }) => {
  for (const entry of API_PAGES) {
    const response = await page.goto(entry.href);
    expect(response?.status(), entry.href).toBe(200);
    await expect(page.getByRole("heading", { level: 1 }), entry.href).toContainText(entry.title);
    const item = page
      .getByRole("navigation", { name: "API reference" })
      .locator(`a[href="${entry.href}"]`);
    await expect(item, entry.href).toHaveAttribute("aria-current", "page");
    if (entry.method) {
      await expect(item, entry.href).toContainText(entry.method);
      // An endpoint page shows its samples: beside the prose on a wide screen,
      // under the summary otherwise — one visible copy either way.
      await expect(
        page.getByText("Request", { exact: true }).filter({ visible: true }),
        entry.href,
      ).toHaveCount(1);
    }
  }
});

test("the section tabs switch between the guides and the reference", async ({ page }) => {
  await page.goto("/docs");
  const tabs = page.getByRole("navigation", { name: "Sections" });
  await expect(tabs.getByRole("link", { name: "Documentation" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await tabs.getByRole("link", { name: "API Reference" }).click();
  await expect(page).toHaveURL(/\/docs\/api$/);
  await expect(tabs.getByRole("link", { name: "API Reference" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(
    page
      .getByRole("navigation", { name: "API reference" })
      .getByRole("link", { name: /Create a deposit request/ }),
  ).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Documentation" })).toHaveCount(0);
});

test("the outline lists the page's own headings and the pager walks the list", async ({ page }) => {
  await page.goto("/docs/concepts");
  const outline = page.getByRole("navigation", { name: "On this page" });
  await expect(outline.getByRole("link", { name: "Lifecycle" })).toBeVisible();
  await expect(outline.getByRole("link", { name: "Where the USDC goes" })).toBeVisible();

  const pager = page.getByRole("navigation", { name: "Adjacent pages" });
  await expect(pager.getByRole("link", { name: /Quickstart/ })).toHaveAttribute(
    "href",
    "/docs/quickstart",
  );
  await pager.getByRole("link", { name: /Verifying the payer/ }).click();
  await expect(page).toHaveURL(/\/docs\/payer-verification$/);
});

test("the reference pager walks the routes in sidebar order", async ({ page }) => {
  await page.goto("/docs/api/deposit-requests/create");
  const pager = page.getByRole("navigation", { name: "Adjacent pages" });
  await expect(pager.getByRole("link", { name: /The deposit request object/ })).toBeVisible();
  await pager.getByRole("link", { name: /List deposit requests/ }).click();
  await expect(page).toHaveURL(/\/docs\/api\/deposit-requests\/list$/);
});

test("choosing a sample language applies to every tab group and survives a reload", async ({
  page,
}) => {
  await page.goto("/docs/quickstart");
  const tabs = page.getByRole("tab", { name: "TypeScript" });
  await tabs.first().click();
  for (const tab of await tabs.all()) await expect(tab).toHaveAttribute("aria-selected", "true");
  await page.reload();
  await expect(page.getByRole("tab", { name: "TypeScript" }).first()).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test("the section list is a drawer and nothing scrolls sideways", async ({ page }) => {
    for (const path of [
      "/docs",
      "/docs/concepts",
      "/docs/api",
      "/docs/api/deposit-requests/create",
      "/docs/api/payer/get",
      "/docs/webhooks",
    ]) {
      await page.goto(path);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
      );
      expect(overflow, path).toBe(false);
    }

    await page.goto("/docs/api");
    await page.getByRole("button", { name: "Open navigation" }).click();
    await page
      .getByRole("navigation", { name: "API reference" })
      .getByRole("link", { name: /Create a customer/ })
      .click();
    await expect(page).toHaveURL(/\/docs\/api\/customers\/create$/);
    await expect(page.getByRole("button", { name: "Open navigation" })).toBeVisible();
  });
});
