/**
 * Smoke tests for the example-app demo.
 *
 * Usage:
 *   1. cargo run -p example-app   (in another terminal)
 *   2. bun run test.ts
 *
 * Or let the script start the server itself:
 *   bun run test.ts --spawn
 */

const BASE = "http://localhost:3000";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function assert(cond: boolean, msg: string) {
  if (!cond) {
    failed++;
    console.error(`  FAIL  ${msg}`);
  } else {
    passed++;
    console.log(`  OK    ${msg}`);
  }
}

async function get(path: string, token: string) {
  return fetch(`${BASE}${path}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
}

async function post(path: string, body: unknown, token: string) {
  return fetch(`${BASE}${path}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(body),
  });
}

// ---------------------------------------------------------------------------
// Server lifecycle (--spawn mode)
// ---------------------------------------------------------------------------

let serverProc: ReturnType<typeof Bun.spawn> | null = null;
let token = "";

async function startServer(): Promise<string> {
  console.log("Building & starting example-app...\n");

  serverProc = Bun.spawn(["cargo", "run", "-p", "example-app"], {
    stdout: "pipe",
    stderr: "inherit",
  });

  // Read stdout line-by-line to capture the JWT printed on startup.
  const reader = serverProc.stdout.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  let jwt = "";

  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });

    // The server prints:
    //   === Test JWT (valid 1h) ===
    //   <token>
    //   (blank line)
    const lines = buf.split("\n");
    for (const line of lines) {
      const trimmed = line.trim();
      // JWT tokens are long base64url strings with dots
      if (trimmed.length > 40 && trimmed.includes(".")) {
        jwt = trimmed;
      }
    }
    if (jwt) break;
  }

  // Release the reader so the process keeps running
  reader.releaseLock();

  if (!jwt) throw new Error("Could not capture JWT from server stdout");

  // Wait for the server to be ready (health endpoint)
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(`${BASE}/health`);
      if (r.ok) return jwt;
    } catch {
      // not ready yet
    }
    await Bun.sleep(500);
  }
  throw new Error("Server did not become ready");
}

function stopServer() {
  if (serverProc) {
    serverProc.kill();
    serverProc = null;
  }
}

// ---------------------------------------------------------------------------
// Obtain the JWT — either from --spawn or from the running server
// ---------------------------------------------------------------------------

async function obtainToken(): Promise<string> {
  if (Bun.argv.includes("--spawn")) {
    return startServer();
  }

  // Assume the server is already running. The user must pass the JWT via
  // env var or we try to reach /health first.
  const envToken = Bun.env.JWT;
  if (envToken) return envToken;

  // Try hitting health to confirm the server is up, then ask for the token.
  try {
    const r = await fetch(`${BASE}/health`);
    if (!r.ok) throw new Error();
  } catch {
    console.error(
      "Server not reachable. Start it first or use: bun run test.ts --spawn"
    );
    process.exit(1);
  }

  console.error("Set the JWT env var:  JWT=<token> bun run test.ts");
  console.error(
    "Or let the script start the server:  bun run test.ts --spawn"
  );
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

async function run() {
  token = await obtainToken();
  console.log(`\nRunning tests against ${BASE}\n`);

  // --- Health ----------------------------------------------------------
  {
    const r = await fetch(`${BASE}/health`);
    assert(r.status === 200, "GET /health → 200");
  }

  // --- POST /users (in-memory) ----------------------------------------
  {
    const r = await post("/users", { name: "Alice", email: "alice@test.dev" }, token);
    assert(r.status === 200, "POST /users → 200");
    const user = await r.json();
    assert(user.name === "Alice", "POST /users → name matches");
    assert(typeof user.id === "number", "POST /users → id is number");
  }

  // --- GET /users ------------------------------------------------------
  {
    const r = await get("/users", token);
    assert(r.status === 200, "GET /users → 200");
    const users = await r.json();
    assert(Array.isArray(users) && users.length >= 1, "GET /users → non-empty array");
  }

  // --- GET /users/1 ----------------------------------------------------
  {
    const r = await get("/users/1", token);
    assert(r.status === 200, "GET /users/1 → 200");
    const user = await r.json();
    assert(user.id === 1, "GET /users/1 → correct id");
  }

  // --- GET /users/9999 (not found) ------------------------------------
  {
    const r = await get("/users/9999", token);
    assert(r.status === 404, "GET /users/9999 → 404");
  }

  // --- GET /me ---------------------------------------------------------
  {
    const r = await get("/me", token);
    assert(r.status === 200, "GET /me → 200");
    const me = await r.json();
    assert(me.sub === "user-123", "GET /me → sub matches");
  }

  // --- GET /admin/users (role-guarded) ---------------------------------
  {
    const r = await get("/admin/users", token);
    assert(r.status === 200, "GET /admin/users → 200 (admin role present)");
  }

  // --- POST /users/db (#[transactional]) — success ---------------------
  {
    const r = await post(
      "/users/db",
      { name: "Bob", email: "bob@test.dev" },
      token
    );
    assert(r.status === 200, "POST /users/db → 200 (transactional commit)");
    const user = await r.json();
    assert(user.name === "Bob", "POST /users/db → name matches");
    assert(user.email === "bob@test.dev", "POST /users/db → email matches");
    assert(typeof user.id === "number" && user.id > 0, "POST /users/db → id assigned");
  }

  // --- POST /users/db again — verify persistence -----------------------
  {
    const r = await post(
      "/users/db",
      { name: "Charlie", email: "charlie@test.dev" },
      token
    );
    assert(r.status === 200, "POST /users/db (2nd) → 200");
    const user = await r.json();
    assert(
      user.id > 1,
      "POST /users/db (2nd) → auto-increment id > 1"
    );
  }

  // --- Unauthorized (no token) -----------------------------------------
  {
    const r = await fetch(`${BASE}/users`);
    assert(r.status !== 200, "GET /users without token → not 200");
  }

  // --- Summary ---------------------------------------------------------
  console.log(`\n${passed + failed} tests — ${passed} passed, ${failed} failed\n`);
  if (failed > 0) process.exit(1);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

try {
  await run();
} finally {
  stopServer();
}
