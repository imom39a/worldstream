// Temporary CI-only file bridge for the daemonized Controller's status probe.
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";

const maxBytes = 16 * 1024;
function validate(record) {
  if (record === null || typeof record !== "object" || Array.isArray(record) ||
      Object.keys(record).sort().join(",") !== "category,stage,status") throw new Error();
  const { stage, status, category } = record;
  if (stage === "membership_status_http") {
    if (!Number.isInteger(status) || status < 100 || status > 599) throw new Error();
    const expected = status === 200 ? "http_success" : status === 429 ? "rate_limited"
      : status === 503 ? "service_unavailable" : status >= 400 && status < 500 ? "request_rejected" : "unexpected_status";
    if (category !== expected) throw new Error();
  } else {
    if (status !== null) throw new Error();
    if (["membership_status_connect", "membership_status_write", "membership_status_read"].includes(stage)) {
      if (!["timeout", "io_failure"].includes(category)) throw new Error();
    } else if (["membership_status_parse", "membership_status_body", "membership_status_validation"].includes(stage)) {
      if (category !== "invalid_response") throw new Error();
    } else throw new Error();
  }
  return { stage, status, category };
}

let descriptor;
try {
  if (process.env.CI !== "true" || process.env.WORLDSTREAM_REENTRY_DIAGNOSTICS !== "visible-local-only") throw new Error();
  const path = process.env.WORLDSTREAM_REENTRY_NATIVE_TRACE_FILE;
  const operation = process.argv[2];
  if (!path || !["prepare", "collect"].includes(operation)) throw new Error();
  descriptor = openSync(path, operation === "prepare"
    ? constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW
    : constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK, 0o600);
  const metadata = fstatSync(descriptor);
  if (!metadata.isFile() || (metadata.mode & 0o777) !== 0o600 || metadata.uid !== process.getuid() ||
      metadata.nlink !== 1 || metadata.size > maxBytes) throw new Error();
  if (operation === "collect") {
    const buffer = Buffer.alloc(maxBytes + 1);
    const length = readSync(descriptor, buffer, 0, buffer.length, 0);
    if (length > maxBytes) throw new Error();
    const content = buffer.subarray(0, length).toString("utf8");
    if (content !== "" && !content.endsWith("\n")) throw new Error();
    const lines = content === "" ? [] : content.slice(0, -1).split("\n");
    if (lines.length > 128) throw new Error();
    // Validate the entire bounded file before writing any record. Never echo
    // unknown keys, arbitrary log text, filesystem paths or exception messages.
    const records = lines.map((line) => validate(JSON.parse(line)));
    for (const record of records) process.stdout.write(`[DEBUG-reentry-native] ${JSON.stringify(record)}\n`);
  }
} catch {
  process.stderr.write("[DEBUG-reentry-native] diagnostic_file_rejected\n");
  process.exitCode = 1;
} finally {
  if (descriptor !== undefined) {
    try { closeSync(descriptor); } catch { process.exitCode = 1; }
  }
}
