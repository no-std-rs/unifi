// All credentials go only to the official cloud origin, in a header.
export const CLOUD_ORIGIN = "https://api.ui.com";
const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;

export function cloudBase(consoleId) {
  if (!/^[a-z0-9:_-]{1,128}$/i.test(consoleId)) throw new Error("Invalid console ID returned by UniFi.");
  const base = `/v1/connector/consoles/${encodeURIComponent(consoleId)}/proxy/network/integration`;
  if ((CLOUD_ORIGIN + base).length > 256) throw new Error("This console URL exceeds the display's settings limit.");
  return base;
}

export function cloudTrust(metadata, now = Date.now()) {
  if (metadata.origin !== CLOUD_ORIGIN || !/^[a-f0-9]{64}$/.test(metadata.certificate_sha256)
      || !Number.isFinite(Date.parse(metadata.not_after)) || Date.parse(metadata.not_after) <= now
      || !Number.isFinite(Date.parse(metadata.verified_at)) || Date.parse(metadata.verified_at) > now + 300000) {
    throw new Error("Cloud certificate information is unavailable or expired. Wait for an updated site build, or use local setup.");
  }
  return metadata.certificate_sha256;
}

export function cloudClient(key, signal, request = fetch) {
  if (!/^[\x21-\x7e]{1,512}$/.test(key)) throw new Error("Enter your Site Manager API key, without spaces.");
  async function get(path) {
    // Callers supply paths, never an origin from an API response or form input.
    const response = await request(CLOUD_ORIGIN + path, {
      method: "GET", headers: { "X-API-Key": key, Accept: "application/json" },
      mode: "cors", credentials: "omit", redirect: "error", cache: "no-store", referrerPolicy: "no-referrer",
      signal: AbortSignal.any([signal, AbortSignal.timeout(20000)]),
    });
    if (!response.ok) {
      const errors = {
        401: "Cloud API key rejected (HTTP 401). Create a key in Site Manager → Settings → API Keys.",
        403: "Cloud access denied (HTTP 403). A personal key can use the Connector only for consoles its owner owns; an organization key needs access to this console.",
        429: "UniFi's cloud request limit was reached (HTTP 429). Wait a minute before retrying.",
      };
      throw new Error(errors[response.status] || `UniFi cloud request failed (HTTP ${response.status}). Check the console is online and runs UniFi OS 5.0.3 or newer.`);
    }
    return response.json();
  }
  async function pages(path) {
    const result = [];
    let offset = 0;
    while (true) {
      const page = await get(`${path}?offset=${offset}&limit=100`);
      if (!Array.isArray(page.data) || !Number.isSafeInteger(page.totalCount) || page.totalCount < 0) throw new Error("Unexpected Network API page.");
      result.push(...page.data);
      offset += page.data.length;
      if (offset >= page.totalCount) return result;
      if (!page.data.length) throw new Error("UniFi returned an incomplete page. Retry discovery.");
    }
  }
  const sitePath = (base, site) => {
    if (!uuid.test(site)) throw new Error("Invalid site UUID returned by UniFi.");
    return `${base}/v1/sites/${site}`;
  };
  return {
    async consoles() {
      const result = [], seen = new Set();
      let next = "";
      while (true) {
        const page = await get(`/v1/hosts${next ? `?nextToken=${encodeURIComponent(next)}` : ""}`);
        if (!Array.isArray(page.data)) throw new Error("Unexpected Site Manager response.");
        result.push(...page.data.filter(h => ["console", "ucore"].includes(h.type) && h.reportedState?.state === "connected"));
        if (!page.nextToken) return result;
        if (typeof page.nextToken !== "string" || seen.has(page.nextToken)) throw new Error("UniFi cloud pagination stalled.");
        next = page.nextToken; seen.add(next);
      }
    },
    sites: base => pages(`${base}/v1/sites`),
    devices: (base, site) => pages(`${sitePath(base, site)}/devices`),
    async verify(base, site, device) {
      if (!uuid.test(device)) throw new Error("Invalid device UUID returned by UniFi.");
      const path = sitePath(base, site);
      // These are the same four reads used by the firmware's first sample.
      for (const suffix of [`/devices/${device}`, `/devices/${device}/statistics/latest`, "/devices?limit=1", "/clients?limit=1"]) {
        await get(path + suffix);
      }
    },
  };
}
