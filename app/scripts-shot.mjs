import { chromium } from "playwright-core";

const seed = JSON.stringify({
  scopes: { low: true, medium: false, high: false },
  folders: {
    "~/Library/Caches": true,
    "/tmp": true,
    "~/Library/Logs": true,
    "~/Library/Application Support (orphaned)": true,
  },
  privacy: "standard",
  done: true,
});

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
await ctx.addInitScript(
  `try{localStorage.setItem('driftwood.app.v1', '${seed}')}catch(e){}`,
);
const page = await ctx.newPage();
page.on("console", (m) => {
  if (m.type() === "error") console.log("CONSOLE ERROR:", m.text());
});
page.on("pageerror", (e) => console.log("PAGE ERROR:", e.message));

for (const [name, url, wait] of [
  ["welcome", "http://localhost:1420/?view=welcome", 2600],
  ["scopes", "http://localhost:1420/?view=scopes", 2600],
  ["trust", "http://localhost:1420/?view=trust", 2600],
  ["privacy", "http://localhost:1420/?view=privacy", 2600],
]) {
  await page.goto(url);
  await page.waitForTimeout(wait);
  await page.screenshot({ path: `shots/${name}.png` });
  console.log("shot", name);
}

// scan: idle → running → auto-navigates to report when the mock finishes
await page.goto("http://localhost:1420/?view=scan");
await page.waitForTimeout(2600);
await page.screenshot({ path: "shots/scan-idle.png" });
console.log("shot scan-idle");

await page.getByRole("button", { name: "Search the river", exact: true }).click();
await page.waitForTimeout(5500);
await page.screenshot({ path: "shots/scan-running.png" });
console.log("shot scan-running");

await page.waitForTimeout(12000); // mock ends ~13.4s + 1.1s finishing + 0.7s page-in
await page.screenshot({ path: "shots/report.png" });
console.log("shot report");

await page.mouse.wheel(0, 700);
await page.waitForTimeout(900);
await page.screenshot({ path: "shots/report-scrolled.png" });
console.log("shot report-scrolled");

const why = page.getByRole("button", { name: "Why the river says so" }).first();
if (await why.count()) {
  await why.click();
  await page.waitForTimeout(700);
  await page.mouse.wheel(0, 420);
  await page.waitForTimeout(500);
  await page.screenshot({ path: "shots/report-expanded.png" });
  console.log("shot report-expanded");
} else {
  console.log("no reasoning buttons found");
}

// snagged (error) state
await page.goto("http://localhost:1420/?view=scan&snag=1");
await page.waitForTimeout(2600);
await page.getByRole("button", { name: "Search the river", exact: true }).click();
await page.waitForTimeout(16000); // error fires ~13.8s in
await page.screenshot({ path: "shots/snagged.png" });
console.log("shot snagged");

await browser.close();
