import assert from "node:assert/strict";
import test from "node:test";

import {
  callbackFault,
  canonicalStringify,
  decodeCanonical,
  encodeCanonical,
  taggedBlake3Text,
  success,
} from "./index.js";

test("canonical JSON sorts keys and round-trips safe integers", () => {
  const bytes = encodeCanonical({ z: 2, a: [true, null, 1] });
  assert.equal(new TextDecoder().decode(bytes), '{"a":[true,null,1],"z":2}');
  assert.deepEqual(decodeCanonical(bytes), { a: [true, null, 1], z: 2 });
});

test("canonical JSON refuses non-canonical and unsafe inputs", () => {
  assert.throws(() => decodeCanonical(new TextEncoder().encode('{"z":2,"a":1}')), /not canonical/);
  assert.throws(() => canonicalStringify(Number.MAX_SAFE_INTEGER + 1), /safe integers/);
});

test("exact UTF-8 BLAKE3 uses the retained tagged wire identity", () => {
  assert.equal(
    taggedBlake3Text("abc"),
    "blake3:6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
  );
});

test("operation envelopes use the frozen tagged shapes", () => {
  assert.deepEqual(success({ ok: true }), {
    operation_result_type: "success",
    output: { ok: true },
  });
  assert.equal(callbackFault("bad\ninput").fault.bounded_safe_detail, "bad input");
});
