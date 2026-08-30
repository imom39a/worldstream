import { readFile, rename, writeFile } from "node:fs/promises";

import { blake3Hex, canonicalBytes, canonicalJson, parseCanonical, type JsonValue } from "./canonical.js";
import {
  BUNDLE_FORMAT_ID,
  CANONICAL_CODEC_ID,
  EXECUTION_PROFILE_ID,
  HOST_CONTRACT_ID,
  REQUIRED_BUNDLE_MEMBERS,
} from "./constants.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import { isRecord } from "./project.js";

const BLOCK = 512;
const MANIFEST = "bundle-manifest.json";

export interface BundleMemberDigest {
  readonly blake3: string;
  readonly name: string;
  readonly size: number;
}

export interface BundleManifest {
  readonly bundle_format_id: typeof BUNDLE_FORMAT_ID;
  readonly canonical_codec: typeof CANONICAL_CODEC_ID;
  readonly execution_profile_id: typeof EXECUTION_PROFILE_ID;
  readonly host_contract: typeof HOST_CONTRACT_ID;
  readonly members: readonly BundleMemberDigest[];
  readonly revision_digest: string;
  readonly static_members: readonly string[];
}

export interface BundleInspection {
  readonly bundleDigest: string;
  readonly manifest: BundleManifest;
  readonly memberNames: readonly string[];
  readonly size: number;
}

export function writePackBundle(
  semanticMembers: ReadonlyMap<string, Uint8Array>,
  revisionDigest: string,
  staticMembers: readonly string[] = [],
): Uint8Array {
  const semanticNames = [...semanticMembers.keys()].sort();
  validateMemberSet(semanticNames, staticMembers);
  const members = semanticNames.map((name) => {
    const bytes = semanticMembers.get(name)!;
    return { blake3: `blake3:${blake3Hex(bytes)}`, name, size: bytes.length };
  });
  const manifest: BundleManifest = {
    bundle_format_id: BUNDLE_FORMAT_ID,
    canonical_codec: CANONICAL_CODEC_ID,
    execution_profile_id: EXECUTION_PROFILE_ID,
    host_contract: HOST_CONTRACT_ID,
    members,
    revision_digest: taggedDigest(revisionDigest),
    static_members: [...staticMembers].sort(),
  };
  const all = new Map(semanticMembers);
  all.set(MANIFEST, canonicalBytes(manifest as unknown as JsonValue));
  return writeCanonicalUstar(all);
}

export async function inspectPackBundle(path: string): Promise<BundleInspection> {
  let bytes: Uint8Array;
  try {
    bytes = new Uint8Array(await readFile(path));
  } catch (error) {
    throw new PackCliError(
      diagnostic("WSP-INSPECT-001", "Cannot read Activity Pack bundle", {
        file: path,
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  }
  try {
    const files = readCanonicalUstar(bytes);
    const manifestBytes = files.get(MANIFEST);
    if (!manifestBytes) throw new Error(`missing ${MANIFEST}`);
    const rawManifest = parseCanonical(manifestBytes);
    const manifest = validateManifest(rawManifest);
    const semanticNames = [...files.keys()].filter((name) => name !== MANIFEST).sort();
    validateMemberSet(semanticNames, manifest.static_members);
    if (canonicalJson(manifest as unknown as JsonValue) !== new TextDecoder().decode(manifestBytes)) {
      throw new Error("bundle manifest is not canonical");
    }
    if (manifest.members.length !== semanticNames.length) {
      throw new Error("manifest member count does not match the archive");
    }
    for (const [index, name] of semanticNames.entries()) {
      const declared = manifest.members[index]!;
      const member = files.get(name)!;
      if (
        declared.name !== name ||
        declared.size !== member.length ||
        declared.blake3 !== `blake3:${blake3Hex(member)}`
      ) {
        throw new Error(`member digest or size mismatch for ${name}`);
      }
    }
    return {
      bundleDigest: `blake3:${blake3Hex(bytes)}`,
      manifest,
      memberNames: [...files.keys()],
      size: bytes.length,
    };
  } catch (error) {
    throw new PackCliError(
      diagnostic("WSP-INSPECT-002", "Activity Pack bundle is not canonical or complete", {
        file: path,
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

export async function finalizePackBundleTranscript(
  path: string,
  revisionDigest: string,
  expectedTranscriptDigest: string,
  actualTranscriptDigest: string,
): Promise<BundleInspection> {
  try {
    for (const [label, value] of [
      ["revision_digest", revisionDigest],
      ["expected_transcript_digest", expectedTranscriptDigest],
      ["actual_transcript_digest", actualTranscriptDigest],
    ] as const) {
      if (!/^blake3:[0-9a-f]{64}$/u.test(value)) {
        throw new TypeError(`${label} is not an exact tagged BLAKE3 identity`);
      }
    }
    if (expectedTranscriptDigest === actualTranscriptDigest) {
      throw new TypeError("needs_finalization proof did not change the transcript identity");
    }

    const originalBytes = new Uint8Array(await readFile(path));
    const originalFiles = readCanonicalUstar(originalBytes);
    const manifestBytes = originalFiles.get(MANIFEST);
    if (!manifestBytes) throw new TypeError(`missing ${MANIFEST}`);
    const manifest = validateManifest(parseCanonical(manifestBytes));
    if (manifest.revision_digest !== revisionDigest) {
      throw new TypeError("host proof revision does not match the draft bundle");
    }
    const corpusBytes = originalFiles.get("golden-corpus.json");
    const conformanceBytes = originalFiles.get("conformance.json");
    if (!corpusBytes || !conformanceBytes) {
      throw new TypeError("draft bundle is missing golden finalization members");
    }
    const corpus = requireObject(parseCanonical(corpusBytes), "golden-corpus.json");
    const conformance = requireObject(parseCanonical(conformanceBytes), "conformance.json");
    if (corpus.expected_transcript_digest !== expectedTranscriptDigest) {
      throw new TypeError("host proof expected digest does not match golden-corpus.json");
    }
    if (conformance.revision_digest !== revisionDigest) {
      throw new TypeError("conformance revision does not match the draft bundle");
    }
    const draftCorpusDigest = `blake3:${blake3Hex(corpusBytes)}`;
    if (conformance.golden_corpus_digest !== draftCorpusDigest) {
      throw new TypeError("conformance golden digest does not match the draft corpus bytes");
    }

    const semanticMembers = new Map(originalFiles);
    semanticMembers.delete(MANIFEST);
    const finalizedCorpus = canonicalBytes({
      ...corpus,
      expected_transcript_digest: actualTranscriptDigest,
    });
    semanticMembers.set("golden-corpus.json", finalizedCorpus);
    semanticMembers.set(
      "conformance.json",
      canonicalBytes({
        ...conformance,
        golden_corpus_digest: `blake3:${blake3Hex(finalizedCorpus)}`,
      }),
    );
    const finalizedBytes = writePackBundle(
      semanticMembers,
      revisionDigest,
      manifest.static_members,
    );
    const finalizedFiles = readCanonicalUstar(finalizedBytes);
    for (const [name, before] of originalFiles) {
      if ([MANIFEST, "golden-corpus.json", "conformance.json"].includes(name)) continue;
      const after = finalizedFiles.get(name);
      if (!after || !sameBytes(before, after)) {
        throw new TypeError(`golden finalization changed immutable member ${name}`);
      }
    }
    const finalizedManifest = validateManifest(
      parseCanonical(finalizedFiles.get(MANIFEST)!),
    );
    if (finalizedManifest.revision_digest !== revisionDigest) {
      throw new TypeError("golden finalization changed the semantic revision");
    }
    const stagingPath = `${path}.finalizing`;
    await writeFile(stagingPath, finalizedBytes, { flag: "wx", mode: 0o600 });
    await rename(stagingPath, path);
    return inspectPackBundle(path);
  } catch (error) {
    if (error instanceof PackCliError) throw error;
    throw new PackCliError(
      diagnostic("WSP-PROVE-005", "Activity Pack golden finalization failed closed", {
        file: path,
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

export function writeCanonicalUstar(files: ReadonlyMap<string, Uint8Array>): Uint8Array {
  const chunks: Uint8Array[] = [];
  const names = [...files.keys()].sort();
  if (new Set(names).size !== names.length) throw new TypeError("duplicate ustar member");
  for (const name of names) {
    validateName(name);
    const contents = files.get(name)!;
    const header = new Uint8Array(BLOCK);
    putText(header, 0, 100, name);
    putOctal(header, 100, 8, 0o644);
    putOctal(header, 108, 8, 0);
    putOctal(header, 116, 8, 0);
    putOctal(header, 124, 12, contents.length);
    putOctal(header, 136, 12, 0);
    header.fill(0x20, 148, 156);
    header[156] = 0x30;
    putText(header, 257, 6, "ustar\0");
    putText(header, 263, 2, "00");
    putOctal(header, 329, 8, 0);
    putOctal(header, 337, 8, 0);
    const checksum = header.reduce((sum, byte) => sum + byte, 0);
    const checksumText = checksum.toString(8).padStart(6, "0");
    putText(header, 148, 8, `${checksumText}\0 `);
    chunks.push(header, contents);
    const padding = (BLOCK - (contents.length % BLOCK)) % BLOCK;
    if (padding > 0) chunks.push(new Uint8Array(padding));
  }
  chunks.push(new Uint8Array(BLOCK * 2));
  return concat(chunks);
}

export function readCanonicalUstar(bytes: Uint8Array): Map<string, Uint8Array> {
  const files = new Map<string, Uint8Array>();
  let offset = 0;
  let previous = "";
  while (offset + BLOCK <= bytes.length) {
    const header = bytes.subarray(offset, offset + BLOCK);
    if (header.every((byte) => byte === 0)) {
      if (offset + BLOCK * 2 !== bytes.length) throw new Error("archive must end with exactly two zero blocks");
      if (!bytes.subarray(offset + BLOCK).every((byte) => byte === 0)) {
        throw new Error("second terminal block is not zeroed");
      }
      const canonical = writeCanonicalUstar(files);
      if (canonical.length !== bytes.length || canonical.some((byte, index) => byte !== bytes[index])) {
        throw new Error("archive headers or padding are not canonical ustar bytes");
      }
      return files;
    }
    validateHeader(header);
    const name = readText(header, 0, 100);
    if (name <= previous) throw new Error("archive members are not strictly name-sorted");
    previous = name;
    validateName(name);
    const size = readOctal(header, 124, 12);
    const start = offset + BLOCK;
    const end = start + size;
    if (end > bytes.length) throw new Error(`truncated member ${name}`);
    files.set(name, bytes.slice(start, end));
    offset = start + Math.ceil(size / BLOCK) * BLOCK;
  }
  throw new Error("archive has no terminal zero blocks");
}

function validateHeader(header: Uint8Array): void {
  const expected = header.slice();
  expected.fill(0x20, 148, 156);
  const actual = readOctal(header, 148, 8);
  const calculated = expected.reduce((sum, byte) => sum + byte, 0);
  if (actual !== calculated) throw new Error("ustar header checksum mismatch");
  if (readOctal(header, 100, 8) !== 0o644) throw new Error("member mode must be 0644");
  if (readOctal(header, 108, 8) !== 0 || readOctal(header, 116, 8) !== 0) {
    throw new Error("member uid/gid must be zero");
  }
  if (readOctal(header, 136, 12) !== 0) throw new Error("member mtime must be zero");
  if (header[156] !== 0x30) throw new Error("only regular files are allowed");
  if (readText(header, 257, 6, true) !== "ustar\0" || readText(header, 263, 2, true) !== "00") {
    throw new Error("archive is not canonical ustar");
  }
  if (readText(header, 157, 100) || readText(header, 265, 32) || readText(header, 297, 32)) {
    throw new Error("linkname, uname, and gname must be empty");
  }
  if (readOctal(header, 329, 8) !== 0 || readOctal(header, 337, 8) !== 0) {
    throw new Error("device numbers must be zero");
  }
  if (readText(header, 345, 155)) throw new Error("ustar prefix must be empty");
}

function validateMemberSet(names: readonly string[], staticMembers: readonly string[]): void {
  const staticSet = new Set(staticMembers);
  if ([...staticSet].some((name) => !name.startsWith("static/")) || staticSet.size !== staticMembers.length) {
    throw new TypeError("static_members must be unique static/* names");
  }
  const allowed = new Set<string>([...REQUIRED_BUNDLE_MEMBERS, ...staticSet]);
  const missing = REQUIRED_BUNDLE_MEMBERS.filter((name) => !names.includes(name));
  const extra = names.filter((name) => !allowed.has(name));
  if (missing.length > 0 || extra.length > 0) {
    throw new TypeError(`bundle members mismatch; missing=${missing.join(",")} extra=${extra.join(",")}`);
  }
}

function validateManifest(value: JsonValue): BundleManifest {
  if (!isRecord(value)) throw new TypeError("bundle manifest must be an object");
  const expectedKeys = [
    "bundle_format_id",
    "canonical_codec",
    "execution_profile_id",
    "host_contract",
    "members",
    "revision_digest",
    "static_members",
  ];
  if (JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(expectedKeys.sort())) {
    throw new TypeError("bundle manifest has unknown or missing fields");
  }
  if (
    value.bundle_format_id !== BUNDLE_FORMAT_ID ||
    value.canonical_codec !== CANONICAL_CODEC_ID ||
    value.execution_profile_id !== EXECUTION_PROFILE_ID ||
    value.host_contract !== HOST_CONTRACT_ID ||
    typeof value.revision_digest !== "string" ||
    !/^blake3:[0-9a-f]{64}$/u.test(value.revision_digest) ||
    !Array.isArray(value.members) ||
    !Array.isArray(value.static_members)
  ) {
    throw new TypeError("bundle manifest identity or field type is invalid");
  }
  const members: BundleMemberDigest[] = value.members.map((item) => {
    if (
      !isRecord(item) ||
      typeof item.name !== "string" ||
      typeof item.size !== "number" ||
      !Number.isSafeInteger(item.size) ||
      typeof item.blake3 !== "string" ||
      !/^blake3:[0-9a-f]{64}$/u.test(item.blake3) ||
      JSON.stringify(Object.keys(item).sort()) !== JSON.stringify(["blake3", "name", "size"])
    ) {
      throw new TypeError("bundle manifest member is invalid");
    }
    return { blake3: item.blake3, name: item.name, size: item.size };
  });
  if (members.some((item, index) => index > 0 && members[index - 1]!.name >= item.name)) {
    throw new TypeError("bundle manifest members are not strictly name-sorted");
  }
  const staticMembers = value.static_members;
  if (staticMembers.some((item) => typeof item !== "string")) {
    throw new TypeError("static_members must contain strings");
  }
  if (
    staticMembers.some(
      (item, index) => index > 0 && String(staticMembers[index - 1]) >= String(item),
    )
  ) {
    throw new TypeError("static_members must be strictly name-sorted");
  }
  return {
    bundle_format_id: BUNDLE_FORMAT_ID,
    canonical_codec: CANONICAL_CODEC_ID,
    execution_profile_id: EXECUTION_PROFILE_ID,
    host_contract: HOST_CONTRACT_ID,
    members,
    revision_digest: value.revision_digest,
    static_members: staticMembers as string[],
  };
}

function validateName(name: string): void {
  const encoded = new TextEncoder().encode(name);
  if (
    name.length === 0 ||
    encoded.length > 100 ||
    name.startsWith("/") ||
    name.includes("..") ||
    name.includes("\\") ||
    name.includes("\0")
  ) {
    throw new TypeError(`invalid canonical bundle member name ${JSON.stringify(name)}`);
  }
}

function requireObject(value: JsonValue, label: string): Record<string, JsonValue> {
  if (!isRecord(value)) throw new TypeError(`${label} must be a canonical JSON object`);
  return value;
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

function taggedDigest(value: string): string {
  const bare = value.startsWith("blake3:") ? value.slice(7) : value;
  if (!/^[0-9a-f]{64}$/u.test(bare)) throw new TypeError("revision digest is not lower-case BLAKE3");
  return `blake3:${bare}`;
}

function putText(target: Uint8Array, offset: number, length: number, value: string): void {
  const bytes = new TextEncoder().encode(value);
  if (bytes.length > length) throw new TypeError("ustar field overflow");
  target.set(bytes, offset);
}

function readText(source: Uint8Array, offset: number, length: number, preserveNull = false): string {
  const bytes = source.subarray(offset, offset + length);
  const nullIndex = bytes.indexOf(0);
  const end = preserveNull || nullIndex < 0 ? bytes.length : nullIndex;
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, end));
}

function putOctal(target: Uint8Array, offset: number, length: number, value: number): void {
  const text = value.toString(8).padStart(length - 1, "0");
  if (text.length !== length - 1) throw new TypeError("ustar numeric field overflow");
  putText(target, offset, length, `${text}\0`);
}

function readOctal(source: Uint8Array, offset: number, length: number): number {
  const text = new TextDecoder().decode(source.subarray(offset, offset + length)).replace(/[\0 ]+$/gu, "");
  if (!/^[0-7]+$/u.test(text)) throw new Error("invalid ustar octal field");
  return Number.parseInt(text, 8);
}

function concat(chunks: readonly Uint8Array[]): Uint8Array {
  const result = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}
