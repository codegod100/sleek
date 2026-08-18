// Unit-test worker.js logic locally (Node has Request/Response/crypto.subtle
// globals matching the Workers runtime closely enough) — no Cloudflare
// account needed. Stubs global fetch (BuildBuddy call) and env.ARTIFACTS
// (R2) so nothing outbound actually happens.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import worker from "./worker.js";

let calls = { fetch: [] };
const realFetch = globalThis.fetch;
globalThis.fetch = async (url, opts) => {
  calls.fetch.push({ url, opts });
  // resolveTagCommit() fetches the tag's own web page (no opts — a plain
  // GET) to scrape its target commit; everything else here is the
  // BuildBuddy Run call. Stub both distinctly so the tag-push test
  // exercises the real resolveTagCommit() parsing path, not just its
  // caller.
  if (!opts && String(url).includes("/tags/")) {
    return new Response(
      `<a href="/nandi.uk/sleek/commit/${MOCK_TAG_COMMIT_SHA}">${MOCK_TAG_COMMIT_SHA.slice(0, 7)}</a>`,
      { status: 200 },
    );
  }
  return new Response(JSON.stringify({ invocationId: "mock-invocation-id" }), { status: 200 });
};

// 40 lowercase-hex chars, like a real git sha — resolveTagCommit()'s regex
// requires exactly that shape.
const MOCK_TAG_COMMIT_SHA = "deadbeef00112233445566778899aabbccddeeff";

class MockBucket {
  constructor() { this.store = new Map(); this.mpus = new Map(); }
  // Minimal stand-in for R2's multipart upload API — enough to exercise
  // worker.js's handleUploadInit/Part/Complete without a real R2 bucket.
  async createMultipartUpload(key) {
    const uploadId = `mock-mpu-${this.mpus.size + 1}`;
    this.mpus.set(uploadId, { key, parts: new Map() });
    return { uploadId, key };
  }
  resumeMultipartUpload(key, uploadId) {
    const mpu = this.mpus.get(uploadId);
    return {
      uploadPart: async (partNumber, body) => {
        const chunks = [];
        const reader = body.getReader();
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          chunks.push(value);
        }
        const buf = Buffer.concat(chunks.map((c) => Buffer.from(c)));
        const etag = `etag-${partNumber}`;
        mpu.parts.set(partNumber, buf);
        return { partNumber, etag };
      },
      complete: async (uploadedParts) => {
        const ordered = [...uploadedParts].sort((a, b) => a.partNumber - b.partNumber);
        const full = Buffer.concat(ordered.map((p) => mpu.parts.get(p.partNumber)));
        this.store.set(key, full);
        this.mpus.delete(uploadId);
      },
    };
  }
  async put(key, body) {
    const chunks = [];
    if (body && body.getReader) {
      const reader = body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
      }
    } else if (typeof body === "string") {
      chunks.push(Buffer.from(body));
    }
    this.store.set(key, Buffer.concat(chunks.map((c) => Buffer.from(c))));
  }
  async delete(key) {
    this.store.delete(key);
  }
  async get(key) {
    const buf = this.store.get(key);
    if (!buf) return null;
    // Real R2Bucket.get() returns an R2ObjectBody whose .body is a
    // ReadableStream (not raw bytes) — match that so put(latestKey,
    // stored.body) exercises the same stream-forwarding path worker.js
    // actually relies on in production.
    return {
      body: new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(buf));
          controller.close();
        },
      }),
      httpEtag: '"mock"',
      writeHttpMetadata(headers) { headers.set("content-type", "application/octet-stream"); },
    };
  }
}

const env = {
  TANGLED_WEBHOOK_SECRET: "test-secret",
  BUILDBUDDY_API_KEY: "test-bb-key",
  UPLOAD_TOKEN: "test-upload-token",
  ARTIFACTS: new MockBucket(),
};

function ctxWithWaitUntil() {
  const pending = [];
  return { ctx: { waitUntil: (p) => pending.push(p) }, pending };
}

function sign(secret, body) {
  return "sha256=" + crypto.createHmac("sha256", secret).update(body).digest("hex");
}

async function testValidPushWebhook() {
  const payload = JSON.stringify({
    after: "abc123",
    ref: "refs/heads/main",
    repository: { clone_url: "https://tangled.org/nandi.uk/sleek" },
  });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: {
      "X-Tangled-Event": "push",
      "X-Tangled-Signature-256": sign("test-secret", payload),
    },
    body: payload,
  });
  const { ctx, pending } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  const text = await res.text();
  assert.match(text, /queued for abc123/);
  await Promise.all(pending);
  assert.equal(calls.fetch.length, 1, "should have triggered exactly one BuildBuddy call");
  const bbBody = JSON.parse(calls.fetch[0].opts.body);
  assert.equal(bbBody.repo, "https://tangled.org/nandi.uk/sleek");
  assert.equal(bbBody.branch, "main");
  assert.equal(bbBody.commit_sha, "abc123", "commit_sha is always sent, even for main pushes");
  assert.equal(bbBody.platform_properties, undefined, "no disk override needed — compute runs on BuildBuddy's RE cluster, not this trigger executor");
  assert.match(bbBody.steps[0].run, /buck2 build --show-output \/\/:sleek-android-apk/);
  assert.equal(calls.fetch[0].opts.headers["x-buildbuddy-api-key"], "test-bb-key");

  const recorded = await env.ARTIFACTS.get("abc123/invocation.json");
  assert.ok(recorded, "should record invocation.json for lookup without a commit-sha filter");
  const recordedBuf = Buffer.concat(await recorded.body.getReader().read().then((r) => [r.value]));
  const recordedJson = JSON.parse(recordedBuf.toString());
  assert.equal(recordedJson.invocationId, "mock-invocation-id");

  console.log("PASS: valid push webhook -> queues BuildBuddy run with correct payload");
}

async function testBadSignatureRejected() {
  const payload = JSON.stringify({ after: "x", ref: "refs/heads/main", repository: { clone_url: "u" } });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: { "X-Tangled-Event": "push", "X-Tangled-Signature-256": "sha256=deadbeef" },
    body: payload,
  });
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 401);
  console.log("PASS: bad signature -> 401");
}

async function testNonMainRefIgnored() {
  const payload = JSON.stringify({ after: "x", ref: "refs/heads/feature", repository: { clone_url: "u" } });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: { "X-Tangled-Event": "push", "X-Tangled-Signature-256": sign("test-secret", payload) },
    body: payload,
  });
  const before = calls.fetch.length;
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  assert.equal(calls.fetch.length, before, "non-main ref must not trigger a build");
  console.log("PASS: non-main ref ignored, no build triggered");
}

async function testNonPushEventIgnored() {
  const payload = JSON.stringify({ after: "x", ref: "refs/heads/main", repository: { clone_url: "u" } });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: { "X-Tangled-Event": "pull_request:created", "X-Tangled-Signature-256": sign("test-secret", payload) },
    body: payload,
  });
  const before = calls.fetch.length;
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  assert.equal(calls.fetch.length, before, "non-push event must not trigger a build");
  console.log("PASS: non-push event ignored");
}

async function testUploadAndDownloadRoundtrip() {
  const body = Buffer.from("fake apk bytes");
  const putReq = new Request("https://proxy.latha.org/upload/deadbeef/sleek.apk", {
    method: "PUT",
    headers: { Authorization: "Bearer test-upload-token" },
    body,
  });
  const { ctx } = ctxWithWaitUntil();
  const putRes = await worker.fetch(putReq, env, ctx);
  assert.equal(putRes.status, 200);

  const getReq = new Request("https://proxy.latha.org/artifacts/deadbeef/sleek.apk");
  const getRes = await worker.fetch(getReq, env, ctx);
  assert.equal(getRes.status, 200);
  const gotBuf = Buffer.from(await getRes.arrayBuffer());
  assert.equal(gotBuf.toString(), "fake apk bytes");

  const latestReq = new Request("https://proxy.latha.org/artifacts/latest/sleek.apk");
  const latestRes = await worker.fetch(latestReq, env, ctx);
  assert.equal(latestRes.status, 200);
  const latestBuf = Buffer.from(await latestRes.arrayBuffer());
  assert.equal(latestBuf.toString(), "fake apk bytes", "latest/ alias must mirror the upload");
  console.log("PASS: upload -> R2 -> download + latest/ alias roundtrip");
}

async function testMultipartUploadRoundtrip() {
  // Simulates the flatpak-upload path buildScript() now uses: init -> N
  // part PUTs -> complete, each request small (the whole point — avoiding
  // the 413 a single 617MB PUT hit against the real Worker).
  const key = "deadbeef/uk.nandi.sleek.flatpak";
  const { ctx } = ctxWithWaitUntil();

  const initReq = new Request(`https://proxy.latha.org/upload-init/${key}`, {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token" },
  });
  const initRes = await worker.fetch(initReq, env, ctx);
  assert.equal(initRes.status, 200);
  const { uploadId } = await initRes.json();
  assert.ok(uploadId);

  const chunkA = Buffer.from("chunk-one-bytes-");
  const chunkB = Buffer.from("chunk-two-bytes-");
  const parts = [];
  let partNumber = 0;
  for (const chunk of [chunkA, chunkB]) {
    partNumber++;
    const partReq = new Request(
      `https://proxy.latha.org/upload-part/${key}?uploadId=${uploadId}&partNumber=${partNumber}`,
      { method: "PUT", headers: { Authorization: "Bearer test-upload-token" }, body: chunk },
    );
    const partRes = await worker.fetch(partReq, env, ctx);
    assert.equal(partRes.status, 200);
    const { etag } = await partRes.json();
    assert.ok(etag);
    parts.push({ partNumber, etag });
  }

  const completeReq = new Request(`https://proxy.latha.org/upload-complete/${key}?uploadId=${uploadId}`, {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token", "content-type": "application/json" },
    body: JSON.stringify(parts),
  });
  const completeRes = await worker.fetch(completeReq, env, ctx);
  assert.equal(completeRes.status, 200);

  const getReq = new Request(`https://proxy.latha.org/artifacts/${key}`);
  const getRes = await worker.fetch(getReq, env, ctx);
  assert.equal(getRes.status, 200);
  const gotBuf = Buffer.from(await getRes.arrayBuffer());
  assert.equal(gotBuf.toString(), Buffer.concat([chunkA, chunkB]).toString(), "reassembled parts must match original bytes in order");

  const latestReq = new Request("https://proxy.latha.org/artifacts/latest/uk.nandi.sleek.flatpak");
  const latestRes = await worker.fetch(latestReq, env, ctx);
  assert.equal(latestRes.status, 200, "multipart complete must also mirror to latest/");

  console.log("PASS: multipart upload init -> parts -> complete -> download + latest/ alias roundtrip");
}

async function testMultipartEndpointsRejectBadToken() {
  const { ctx } = ctxWithWaitUntil();
  const initReq = new Request("https://proxy.latha.org/upload-init/x/y", {
    method: "POST",
    headers: { Authorization: "Bearer wrong" },
  });
  const res = await worker.fetch(initReq, env, ctx);
  assert.equal(res.status, 401);
  console.log("PASS: upload-init with bad bearer token -> 401");
}

async function testUploadRejectsBadToken() {
  const req = new Request("https://proxy.latha.org/upload/x/sleek.apk", {
    method: "PUT",
    headers: { Authorization: "Bearer wrong" },
    body: "nope",
  });
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 401);
  console.log("PASS: upload with bad bearer token -> 401");
}

async function testDownloadMissingKey404() {
  const req = new Request("https://proxy.latha.org/artifacts/nope/sleek.apk");
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 404);
  console.log("PASS: download of missing key -> 404");
}

async function testOauthSessionNeverServedAsArtifact() {
  // The whole point of the oauth/ prefix guard in handleDownload: even if
  // an oauth session file exists in the bucket, /artifacts/oauth/... must
  // never serve it.
  await env.ARTIFACTS.put("oauth/session.json", JSON.stringify({ accessToken: "should-never-leak" }));
  const req = new Request("https://proxy.latha.org/artifacts/oauth/session.json");
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 404, "oauth/ prefix must never be servable via /artifacts/");
  await env.ARTIFACTS.delete("oauth/session.json");
  console.log("PASS: /artifacts/oauth/... is always 404, never leaks the session");
}

async function testClientMetadataDocument() {
  const req = new Request("https://proxy.latha.org/client-metadata.json");
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "application/json");
  const meta = await res.json();
  assert.equal(meta.client_id, "https://proxy.latha.org/client-metadata.json", "client_id must exactly match the URL it's served from");
  assert.deepEqual(meta.redirect_uris, ["https://proxy.latha.org/oauth/callback"]);
  assert.equal(meta.dpop_bound_access_tokens, true);
  assert.ok(meta.scope.includes("atproto"));
  console.log("PASS: /client-metadata.json is well-formed and self-referential");
}

async function testTagPushTriggersBuildAndPublishStep() {
  const payload = JSON.stringify({
    after: "tagcommitsha",
    ref: "refs/tags/v1.2.3",
    repository: { clone_url: "https://tangled.org/nandi.uk/sleek" },
  });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: { "X-Tangled-Event": "push", "X-Tangled-Signature-256": sign("test-secret", payload) },
    body: payload,
  });
  const before = calls.fetch.length;
  const { ctx, pending } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  const text = await res.text();
  assert.match(text, /tag v1\.2\.3/);
  await Promise.all(pending);
  // 2 calls: resolveTagCommit()'s GET of the tag's own page (scraping its
  // target commit — payload.after is the *tag object*'s sha, not a
  // commit, confirmed against a real Tangled delivery) + the actual
  // BuildBuddy Run call.
  assert.equal(calls.fetch.length, before + 2, "tag push must resolve the tag's commit, then trigger exactly one BuildBuddy call");
  const tagPageCall = calls.fetch[before];
  assert.match(String(tagPageCall.url), /\/tags\/v1\.2\.3$/, "must scrape the pushed tag's own page, not a hardcoded one");
  const bbBody = JSON.parse(calls.fetch[calls.fetch.length - 1].opts.body);
  assert.equal(bbBody.commit_sha, MOCK_TAG_COMMIT_SHA, "commit_sha must be the tag's *resolved commit*, not payload.after (which is the tag object's own sha for annotated tags)");
  assert.equal(bbBody.branch, undefined, "no branch field for tag pushes — a tag name isn't a valid checkout branch and would hit BuildBuddy's origin/<ref> checkout bug");
  assert.doesNotMatch(
    bbBody.steps[0].run,
    /git rev-parse "refs\/tags/,
    "must not try to resolve the tag ref via git on the trigger executor — that ref is never fetched there (only commit_sha's single commit object is)",
  );
  // The curl -d payload is itself shell-quoted (it runs inside the outer
  // `-d "..."` argument), so its JSON quotes are backslash-escaped in the
  // literal script text: \"sha\":\"<sha>\" rather than "sha":"<sha>".
  assert.match(bbBody.steps[0].run, new RegExp(`\\\\"sha\\\\":\\\\"${MOCK_TAG_COMMIT_SHA}\\\\"`), "release-publish step reports the resolved commit sha, not the tag object sha");
  assert.match(
    bbBody.steps[0].run,
    new RegExp(`\\\\"tagHash\\\\":\\\\"tagcommitsha\\\\"`),
    "release-publish step reports payload.after (the tag object's own sha) as tagHash, as a literal — not a git rev-parse result",
  );
  assert.match(bbBody.steps[0].run, /\/publish-release\/v1\.2\.3/, "build script calls back to publish the release");
  console.log("PASS: tag push -> resolves the tag's real commit, builds with commit_sha (not branch), and appends a release-publish step");
}

async function testTagPushBuildScriptIncludesHostBinary() {
  // A tag push must also build //:sleek-host (the desktop egui binary
  // codegod100/tap's Formula/sleek.rb downloads) and publish it as a
  // second, separately-named artifact under the same tag — not just the
  // apk. See buildScript()'s tagName branch.
  const payload = JSON.stringify({
    after: "tagcommitsha2",
    ref: "refs/tags/v4.5.6",
    repository: { clone_url: "https://tangled.org/nandi.uk/sleek" },
  });
  const req = new Request("https://proxy.latha.org/webhook", {
    method: "POST",
    headers: { "X-Tangled-Event": "push", "X-Tangled-Signature-256": sign("test-secret", payload) },
    body: payload,
  });
  const { ctx, pending } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 200);
  await Promise.all(pending);
  const bbBody = JSON.parse(calls.fetch[calls.fetch.length - 1].opts.body);
  const script = bbBody.steps[0].run;
  assert.match(script, /buck2 build --show-output \/\/:sleek-host/, "must also build the desktop host binary");
  assert.match(script, new RegExp(`upload/${MOCK_TAG_COMMIT_SHA}/sleek-x86_64-linux`), "must upload it under its own filename, not overwrite sleek.apk");
  // Two release-publish calls: one per artifact, each naming which file it's
  // publishing — filename defaults to sleek.apk server-side, so the apk
  // call must still say so explicitly for the second one to be
  // distinguishable at all.
  const publishCalls = script.match(/\/publish-release\/v4\.5\.6/g) || [];
  assert.equal(publishCalls.length, 2, "must publish both the apk and the host binary as separate release records");
  assert.match(script, /\\"filename\\":\\"sleek\.apk\\"/, "apk publish call must name itself explicitly");
  assert.match(script, /\\"filename\\":\\"sleek-x86_64-linux\\"/, "host-binary publish call must name itself");
  console.log("PASS: tag push build script also builds + publishes the desktop host binary as a distinct artifact");
}

async function testPublishReleaseRejectsBadToken() {
  const req = new Request("https://proxy.latha.org/publish-release/v1.2.3", {
    method: "POST",
    headers: { Authorization: "Bearer wrong", "content-type": "application/json" },
    body: JSON.stringify({ sha: "x", tagHash: "y" }),
  });
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 401);
  console.log("PASS: publish-release with bad bearer token -> 401");
}

async function testPublishReleaseMissingArtifact404() {
  const req = new Request("https://proxy.latha.org/publish-release/v1.2.3", {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token", "content-type": "application/json" },
    body: JSON.stringify({ sha: "never-uploaded-sha", tagHash: "deadbeef" }),
  });
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 404);
  console.log("PASS: publish-release for a sha with no stored apk -> 404 (not a crash)");
}

async function testPublishReleaseWithoutOauthSession401() {
  // apk is present but nobody has ever completed /oauth/login -> must fail
  // clearly, not crash, and must not attempt any atproto network call.
  const key = "releasesha/sleek.apk";
  await env.ARTIFACTS.put(key, "fake apk bytes for release test");
  const before = calls.fetch.length;
  const req = new Request("https://proxy.latha.org/publish-release/v9.9.9", {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token", "content-type": "application/json" },
    body: JSON.stringify({ sha: "releasesha", tagHash: "deadbeef" }),
  });
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 401);
  assert.match(await res.text(), /oauth\/login/);
  assert.equal(calls.fetch.length, before, "must not call out to atproto without a session");
  console.log("PASS: publish-release without a prior /oauth/login -> 401, no atproto calls attempted");
}

async function testPublishReleaseWithExplicitFilenameDoesNotClobber() {
  // Two artifacts published under the same tag (apk defaults to
  // "sleek.apk"; the host binary passes filename explicitly) must land as
  // two separate releases/<tag>/<filename>.json records, not one
  // overwriting the other.
  await env.ARTIFACTS.put("multisha/sleek.apk", "fake apk bytes");
  await env.ARTIFACTS.put("multisha/sleek-x86_64-linux", "fake host binary bytes");
  // No oauth session configured in this test env, so both calls 401 before
  // ever reaching atproto — enough to prove each call resolves and records
  // against its *own* filename-keyed R2 object without a crash or a wrong
  // "no artifact stored" 404 for either one.
  const reqApk = new Request("https://proxy.latha.org/publish-release/v7.7.7", {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token", "content-type": "application/json" },
    body: JSON.stringify({ sha: "multisha", tagHash: "deadbeef", filename: "sleek.apk" }),
  });
  const reqHost = new Request("https://proxy.latha.org/publish-release/v7.7.7", {
    method: "POST",
    headers: { Authorization: "Bearer test-upload-token", "content-type": "application/json" },
    body: JSON.stringify({ sha: "multisha", tagHash: "deadbeef", filename: "sleek-x86_64-linux" }),
  });
  const { ctx } = ctxWithWaitUntil();
  const resApk = await worker.fetch(reqApk, env, ctx);
  const resHost = await worker.fetch(reqHost, env, ctx);
  assert.equal(resApk.status, 401, "apk artifact was found (not 404) — just no oauth session");
  assert.equal(resHost.status, 401, "host-binary artifact was found (not 404) — just no oauth session");
  console.log("PASS: publish-release resolves each artifact by its own filename, not just sha");
}

async function testOauthCallbackRejectsUnknownState() {
  const req = new Request("https://proxy.latha.org/oauth/callback?code=abc&state=never-issued");
  const { ctx } = ctxWithWaitUntil();
  const res = await worker.fetch(req, env, ctx);
  assert.equal(res.status, 400);
  console.log("PASS: oauth callback with unknown state -> 400, not a crash");
}

const tests = [
  testValidPushWebhook,
  testBadSignatureRejected,
  testNonMainRefIgnored,
  testNonPushEventIgnored,
  testUploadAndDownloadRoundtrip,
  testMultipartUploadRoundtrip,
  testMultipartEndpointsRejectBadToken,
  testUploadRejectsBadToken,
  testDownloadMissingKey404,
  testOauthSessionNeverServedAsArtifact,
  testClientMetadataDocument,
  testOauthCallbackRejectsUnknownState,
  testTagPushTriggersBuildAndPublishStep,
  testTagPushBuildScriptIncludesHostBinary,
  testPublishReleaseRejectsBadToken,
  testPublishReleaseMissingArtifact404,
  testPublishReleaseWithoutOauthSession401,
  testPublishReleaseWithExplicitFilenameDoesNotClobber,
];

let failed = 0;
for (const t of tests) {
  try {
    await t();
  } catch (e) {
    failed++;
    console.error(`FAIL: ${t.name}:`, e.message);
  }
}
globalThis.fetch = realFetch;
console.log(failed === 0 ? `\nALL ${tests.length} TESTS PASSED` : `\n${failed}/${tests.length} TESTS FAILED`);
process.exit(failed === 0 ? 0 : 1);
