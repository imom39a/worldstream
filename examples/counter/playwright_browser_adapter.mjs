/** Standalone local/CI adapter. Importing it does not launch a browser. */

export function renderedBodyText(page) {
  return page.locator("body").innerText();
}

export async function createPlaywrightBrowserAdapter({ studioUrl, fixture }) {
  if (typeof studioUrl !== "string" || !studioUrl.startsWith("http://127.0.0.1:")) {
    throw new TypeError("studioUrl must be a loopback Studio URL");
  }
  if (fixture === undefined || fixture === null) throw new TypeError("fixture is required");
  const { chromium } = await import("playwright");
  const browser = await chromium.launch();
  const context = await browser.newContext();
  const studioPage = await context.newPage();
  const tabs = new WeakMap();
  const observe = (page) => {
    const retained = tabs.get(page);
    if (retained) return retained;
    const observed = [];
    page.on("request", (request) => observed.push(request.url()));
    const tab = {
      playwright: page,
      domSnapshot: () => renderedBodyText(page),
      observedRequestUrls: () => [...observed],
    };
    tabs.set(page, tab);
    return tab;
  };
  context.on("page", observe);
  const studioTab = observe(studioPage);
  await studioPage.goto(studioUrl, { waitUntil: "domcontentloaded" });
  const initialPages = new Set(context.pages());
  return {
    tab: studioTab,
    fixture,
    async beforeOpenProtectedConsole() {},
    async openProtectedConsole() {
      const deadline = Date.now() + 10_000;
      while (Date.now() < deadline) {
        const candidate = context.pages().find((page) => !initialPages.has(page));
        if (candidate) {
          await candidate.waitForLoadState("domcontentloaded");
          return observe(candidate);
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      throw new Error("protected_console_popup_not_exactly_one");
    },
    async observedRequestUrls(tab) {
      if (typeof tab?.observedRequestUrls !== "function") {
        throw new Error("browser_request_url_evidence_required");
      }
      return tab.observedRequestUrls();
    },
    async currentUrl(tab) {
      return tab?.playwright?.url();
    },
    async close() {
      await browser.close();
    },
  };
}
