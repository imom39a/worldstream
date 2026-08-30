import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  finalizePackBundleTranscript,
  inspectPackBundle,
  readCanonicalUstar,
  writePackBundle,
} from "./bundle.js";
import { canonicalBytes, parseCanonical, taggedBlake3 } from "./canonical.js";
import { REQUIRED_BUNDLE_MEMBERS } from "./constants.js";

test("canonical bundle writer is deterministic and self-verifying", async () => {
  const members = new Map(
    REQUIRED_BUNDLE_MEMBERS.map((name) => [
      name,
      name === "executor.component.wasm"
        ? new Uint8Array([0, 97, 115, 109])
        : canonicalBytes({ member: name }),
    ]),
  );
  const revision = `blake3:${"a".repeat(64)}`;
  const first = writePackBundle(members, revision);
  const second = writePackBundle(new Map([...members].reverse()), revision);
  assert.deepEqual(first, second);
  const directory = await mkdtemp(join(tmpdir(), "worldstream-bundle-test-"));
  const path = join(directory, "fixture.wspack");
  await writeFile(path, first);
  const inspection = await inspectPackBundle(path);
  assert.equal(inspection.manifest.revision_digest, `blake3:${"a".repeat(64)}`);
  assert.match(inspection.manifest.members[0]!.blake3, /^blake3:[0-9a-f]{64}$/u);
  assert.deepEqual(
    inspection.manifest.members.map((member) => member.name),
    [...REQUIRED_BUNDLE_MEMBERS].sort(),
  );
});

test("bundle inspection rejects trailing bytes", async () => {
  const members = new Map(
    REQUIRED_BUNDLE_MEMBERS.map((name) => [name, canonicalBytes({ member: name })]),
  );
  const valid = writePackBundle(members, "b".repeat(64));
  const malformed = new Uint8Array(valid.length + 1);
  malformed.set(valid);
  const directory = await mkdtemp(join(tmpdir(), "worldstream-bundle-test-"));
  const path = join(directory, "trailing.wspack");
  await writeFile(path, malformed);
  await assert.rejects(() => inspectPackBundle(path), /Activity Pack bundle/);
});

test("golden finalization preserves the exact Component and semantic revision", async () => {
  const revision = `blake3:${"a".repeat(64)}`;
  const expected = `blake3:${"b".repeat(64)}`;
  const actual = `blake3:${"c".repeat(64)}`;
  const corpus = canonicalBytes({
    corpus_id: "worldstream/pack-golden-corpus/v1",
    expected_transcript_digest: expected,
  });
  const conformance = canonicalBytes({
    conformance_id: "worldstream/pack-conformance/v1",
    golden_corpus_digest: taggedBlake3(corpus),
    revision_digest: revision,
  });
  const component = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
  const members = new Map(
    REQUIRED_BUNDLE_MEMBERS.map((name) => [
      name,
      name === "executor.component.wasm"
        ? component
        : name === "golden-corpus.json"
          ? corpus
          : name === "conformance.json"
            ? conformance
            : canonicalBytes({ member: name }),
    ]),
  );
  const directory = await mkdtemp(join(tmpdir(), "worldstream-finalize-test-"));
  const path = join(directory, "draft.wspack");
  const draft = writePackBundle(members, revision);
  await writeFile(path, draft);

  const finalized = await finalizePackBundleTranscript(path, revision, expected, actual);
  assert.equal(finalized.manifest.revision_digest, revision);
  assert.notEqual(finalized.bundleDigest, taggedBlake3(draft));
  const files = readCanonicalUstar(new Uint8Array(await readFile(path)));
  assert.deepEqual(files.get("executor.component.wasm"), component);
  const finalizedCorpus = parseCanonical(files.get("golden-corpus.json")!) as Record<
    string,
    unknown
  >;
  assert.equal(finalizedCorpus.expected_transcript_digest, actual);
  const finalizedConformance = parseCanonical(files.get("conformance.json")!) as Record<
    string,
    unknown
  >;
  assert.equal(
    finalizedConformance.golden_corpus_digest,
    taggedBlake3(files.get("golden-corpus.json")!),
  );
});
