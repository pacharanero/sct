// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// From the repository root, serve the unmodified application asset:
// python3 -m http.server 18421 --bind 127.0.0.1 --directory assets
// Run Playwright MCP browser_run_code_unsafe with filename set to the absolute
// path of this file. No npm packages, running SCT server, or licensed data needed.
// Only the initial asset fetch reaches a server. All test-page requests, including
// CDN scripts/styles, are intercepted. Stop the asset server when finished.

async (page) => {
  const asset = await page.request.get("http://127.0.0.1:18421/index.html");
  if (!asset.ok()) throw new Error(`GUI asset fetch failed: ${asset.status()}`);
  const html = await asset.text();
  if (!html.includes("function renderConcept(")) {
    throw new Error("Asset server must serve this checkout's assets/index.html");
  }

  const context = await page.context().browser().newContext();
  const origin = "http://127.0.0.1:18422";
  const payload = "');window.__guiBoundaryExecuted=true;//\"<svg onload=window.__guiBoundaryExecuted=true>\\&?#";
  const display = "<img src=x onerror=window.__guiBoundaryExecuted=true>";
  const passed = [];
  let ids;
  let requests;
  let unexpected;
  let pageErrors;

  const apiUrl = (kind, id) => `${origin}/api/${kind}/${encodeURIComponent(id)}`;
  const assert = (condition, message) => {
    if (!condition) throw new Error(message);
  };

  try {
    await context.addInitScript(() => { window.__guiBoundaryExecuted = false; });
    await context.route("**/*", async route => {
      const request = route.request();
      const url = request.url();
      if (!url.startsWith(`${origin}/`)) {
        // The detail view needs neither CDN styling nor D3. Never contact a CDN.
        if (["script", "stylesheet"].includes(request.resourceType())) {
          return route.fulfill({
            status: 200,
            contentType: request.resourceType() === "script" ? "text/javascript" : "text/css",
            body: "",
          });
        }
        unexpected.push(request.url());
        return route.abort();
      }
      const pathname = url.slice(origin.length).split("?")[0];
      if (pathname === "/") {
        return route.fulfill({ status: 200, contentType: "text/html", body: html });
      }
      requests.push(request.url());
      let body;
      if (pathname === "/api/hierarchy") {
        body = { hierarchies: [] };
      } else if (pathname === "/api/search") {
        body = { total: 1, results: [{ id: ids.focal, preferred_term: "Focal result" }] };
      } else if (request.url() === apiUrl("concept", ids.focal)) {
        body = {
          id: ids.focal,
          preferred_term: "Focal concept",
          fsn: display,
          synonyms: [display],
          parents: [{ id: ids.parent, fsn: "Parent target" }],
          attributes: { site: [{ id: ids.attribute, fsn: "Attribute target" }] },
          children_count: 1,
        };
      } else if ([ids.parent, ids.attribute, ids.child].some(id => request.url() === apiUrl("concept", id))) {
        body = {
          id: decodeURIComponent(pathname.slice("/api/concept/".length)),
          preferred_term: "Reached target",
          children_count: 0,
        };
      } else if (request.url() === apiUrl("children", ids.focal)) {
        body = { children: [{ id: ids.child, preferred_term: "Child target" }] };
      } else if (pathname.startsWith("/api/size/")) {
        body = { sqlite_human: "1 KB", ndjson_human: "1 KB" };
      } else {
        unexpected.push(request.url());
        return route.abort();
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    });

    const browserPage = await context.newPage();
    browserPage.on("pageerror", error => pageErrors.push(error.message));
    const clickRequest = async (button, expected) => {
      // A missing bindConceptButtons call must fail, not merely leave a safe DOM.
      await Promise.all([
        browserPage.waitForRequest(request => request.url() === expected, { timeout: 3000 }),
        button.click({ timeout: 3000 }),
      ]);
    };
    const assertSafe = async () => {
      assert(await browserPage.evaluate(() => window.__guiBoundaryExecuted === false), "Injected JavaScript executed");
      assert(await browserPage.locator("#concept-detail img, #concept-detail svg, #concept-detail script").count() === 0,
        "Display data created an HTML element");
      assert(unexpected.length === 0, `Unexpected request(s): ${JSON.stringify(unexpected)}`);
      assert(pageErrors.length === 0, `Browser error(s): ${JSON.stringify(pageErrors)}`);
    };

    for (const malicious of [false, true]) {
      for (const site of ["parent", "attribute", "show-children", "child"]) {
        const scenario = `${malicious ? "malicious" : "numeric"}/${site}`;
        ids = Object.fromEntries(Object.entries({
          focal: "22298006", parent: "404684003", attribute: "74281007", child: "46635009",
        }).map(([key, value]) => [key, malicious ? value + payload : value]));
        requests = [];
        unexpected = [];
        pageErrors = [];
        try {
          await browserPage.goto(origin, { waitUntil: "networkidle" });
          await browserPage.locator("#search-input").fill("Focal result");
          await clickRequest(browserPage.locator("#results-list .result-item"), apiUrl("concept", ids.focal));
          await browserPage.getByRole("heading", { name: "Focal concept", exact: true }).waitFor();
          await assertSafe();

          if (site === "parent" || site === "attribute") {
            const label = site === "parent" ? "Parent target" : "Attribute target";
            await clickRequest(browserPage.getByRole("button", { name: label, exact: true }), apiUrl("concept", ids[site]));
          } else {
            await clickRequest(browserPage.getByRole("button", { name: "Show children", exact: true }), apiUrl("children", ids.focal));
            const child = browserPage.locator("#children-container [data-concept-id]");
            await child.waitFor();
            assert(await child.getAttribute("data-concept-id") === ids.child, "Child ID changed during HTML parsing");
            if (site === "child") await clickRequest(child, apiUrl("concept", ids.child));
          }
          if (site !== "show-children") {
            await browserPage.getByRole("heading", { name: "Reached target", exact: true }).waitFor();
          }
          await browserPage.waitForLoadState("networkidle");
          await assertSafe();
          const actual = requests.filter(url => url.startsWith(`${origin}/api/concept/`) || url.startsWith(`${origin}/api/children/`));
          const expected = [apiUrl("concept", ids.focal)];
          if (site === "show-children" || site === "child") expected.push(apiUrl("children", ids.focal));
          if (site !== "show-children") expected.push(apiUrl("concept", ids[site]));
          assert(JSON.stringify(actual) === JSON.stringify(expected),
            `IDs did not round-trip through request URLs: ${JSON.stringify({ actual, expected })}`);
          passed.push(scenario);
        } catch (error) {
          throw new Error(`${scenario}: ${error.message}`);
        }
      }
    }
    return { passed, scenarios: passed.length, injectedJavaScriptExecuted: false };
  } finally {
    await context.close();
  }
}
