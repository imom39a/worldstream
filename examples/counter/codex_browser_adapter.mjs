/** Adapter for the Codex in-app browser runtime; it never opens a browser itself. */

function requireValue(value, name) {
  if (value === undefined || value === null) throw new TypeError(`${name} is required`);
}

async function openTabs(browser) {
  if (typeof browser?.user?.openTabs !== "function") {
    throw new TypeError("Codex browser runtime must provide user.openTabs()");
  }
  const tabs = await browser.user.openTabs();
  if (!Array.isArray(tabs)) throw new TypeError("Codex browser returned invalid open tabs");
  return tabs;
}

function tabId(tab) {
  return tab?.id ?? tab?.tabId;
}

function browserTab(tab) {
  const snapshot = typeof tab?.domSnapshot === "function"
    ? () => tab.domSnapshot()
    : typeof tab?.playwright?.domSnapshot === "function"
      ? () => tab.playwright.domSnapshot()
      : undefined;
  if (!snapshot || !tab?.playwright) {
    throw new TypeError("Codex tab must provide playwright.domSnapshot()");
  }
  return {
    rawTab: tab,
    id: tab.id,
    tabId: tab.tabId,
    playwright: tab.playwright,
    domSnapshot: snapshot,
  };
}

async function claimTab(browser, tab) {
  if (tab?.playwright) return tab;
  const id = tabId(tab);
  if (typeof browser?.user?.claimTab !== "function" || id === undefined) {
    throw new TypeError("Codex browser runtime must provide user.claimTab(tab.id)");
  }
  return browser.user.claimTab(id);
}

function consoleOrigin(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port) throw new Error();
    return url.origin;
  } catch {
    throw new TypeError("consoleOrigin must be a loopback HTTP origin");
  }
}

function titleOf(tab) {
  return typeof tab?.title === "string" ? tab.title.trim() : "";
}

async function readyPopup(tab, currentUrlFor, expectedOrigin, titleFor) {
  if (!String(await titleFor(tab) ?? "").trim()) return false;
  try {
    return new URL(await currentUrlFor(tab)).origin === expectedOrigin;
  } catch {
    return false;
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/**
 * Builds the production in-app adapter around a caller-selected Studio tab.
 * `requestUrlsFor` is deliberately mandatory: acceptance fails closed when the
 * selected browser runtime cannot provide bounded request URL evidence.
 */
export function createCodexBrowserAdapter({
  browser,
  studioTab,
  fixture,
  beginRequestCapture,
  captureTab,
  requestUrlsFor,
  currentUrlFor,
  consoleOrigin: configuredConsoleOrigin = "http://127.0.0.1:5173",
  titleFor = titleOf,
  popupTimeoutMs = 10_000,
  popupPollMs = 50,
}) {
  requireValue(browser, "browser");
  requireValue(studioTab, "studioTab");
  requireValue(fixture, "fixture");
  if (typeof beginRequestCapture !== "function" || typeof captureTab !== "function" || typeof requestUrlsFor !== "function") {
    throw new TypeError("beginRequestCapture(), captureTab(), and requestUrlsFor(tab) are required for browser disclosure evidence");
  }
  if (typeof currentUrlFor !== "function") {
    throw new TypeError("currentUrlFor(tab) is required for browser URL evidence");
  }
  if (typeof titleFor !== "function") {
    throw new TypeError("titleFor(tab) must provide bounded popup readiness evidence");
  }
  if (!Number.isInteger(popupTimeoutMs) || popupTimeoutMs < 1_000
      || !Number.isInteger(popupPollMs) || popupPollMs < 1) {
    throw new TypeError("popup polling bounds are invalid");
  }
  const expectedConsoleOrigin = consoleOrigin(configuredConsoleOrigin);
  let knownIds;
  const captures = new Map();
  const captureFor = (tab) => captures.get(tabId(tab));
  return {
    tab: browserTab(studioTab),
    fixture,
    async beforeOpenProtectedConsole() {
      const capture = await beginRequestCapture(browser, studioTab);
      if (capture === undefined || capture === null) throw new Error("studio_request_capture_unavailable");
      captures.set(tabId(studioTab), capture);
      knownIds = new Set((await openTabs(browser)).map(tabId));
    },
    async openProtectedConsole() {
      const deadline = Date.now() + popupTimeoutMs;
      do {
        const candidates = (await openTabs(browser)).filter((tab) => !knownIds?.has(tabId(tab)));
        if (candidates.length > 1) throw new Error("protected_console_popup_not_exactly_one");
        if (candidates.length === 1 && await readyPopup(candidates[0], currentUrlFor, expectedConsoleOrigin, titleFor)) {
          const claimed = browserTab(await claimTab(browser, candidates[0]));
          const capture = await captureTab(claimed.rawTab, captureFor(studioTab));
          if (capture === undefined || capture === null) throw new Error("console_request_capture_unavailable");
          captures.set(tabId(claimed.rawTab), capture);
          claimed.observedRequestUrls = () => requestUrlsFor(claimed.rawTab, capture);
          return claimed;
        }
        await delay(popupPollMs);
      } while (Date.now() < deadline);
      throw new Error("protected_console_popup_not_ready");
    },
    async observedRequestUrls(tab) {
      const rawTab = tab?.rawTab ?? tab;
      const capture = captureFor(rawTab);
      if (capture === undefined) throw new Error("browser_request_capture_not_started");
      const urls = await requestUrlsFor(rawTab, capture);
      if (!Array.isArray(urls) || urls.some((url) => typeof url !== "string")) {
        throw new Error("browser_request_url_evidence_invalid");
      }
      return urls;
    },
    async currentUrl(tab) {
      const url = await currentUrlFor(tab?.rawTab ?? tab);
      if (typeof url !== "string") throw new Error("browser_current_url_evidence_invalid");
      return url;
    },
  };
}
