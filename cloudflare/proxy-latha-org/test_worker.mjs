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
  return new Response(JSON.stringify({ invocationId: "mock-invocation-id" }), { status: 200 });
};

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
  assert.equal(bbBody.platform_properties.EstimatedFreeDiskBytes, "60GB");
  assert.match(bbBody.steps[0].run, /nix build \.#android/);
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
