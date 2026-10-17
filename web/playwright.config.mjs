import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests", testMatch: "browser.spec.mjs", fullyParallel: true,
  use: { baseURL: "http://127.0.0.1:4173", headless: true },
  webServer: [
    { command: "node tests/server.mjs", url: "http://127.0.0.1:4173", reuseExistingServer: false },
    { command: "python3 ../tools/setup-display.py --no-browser", url: "http://127.0.0.1:4177", reuseExistingServer: false },
  ],
});
