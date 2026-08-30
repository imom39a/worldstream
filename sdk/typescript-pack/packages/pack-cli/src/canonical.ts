import { blake3 } from "@noble/hashes/blake3.js";
import { bytesToHex } from "@noble/hashes/utils.js";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

const encoder = new TextEncoder();

export function canonicalJson(value: JsonValue): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) {
      throw new TypeError("canonical JSON permits safe integers only");
    }
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key]!)}`)
    .join(",")}}`;
}

export function canonicalBytes(value: JsonValue): Uint8Array {
  return encoder.encode(canonicalJson(value));
}

export function blake3Hex(bytes: Uint8Array): string {
  return bytesToHex(blake3(bytes));
}

export function taggedBlake3(bytes: Uint8Array): string {
  return `blake3:${blake3Hex(bytes)}`;
}

export function parseCanonical(bytes: Uint8Array): JsonValue {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  const value = JSON.parse(text) as JsonValue;
  if (canonicalJson(value) !== text) {
    throw new TypeError("JSON bytes are not canonical");
  }
  return value;
}
