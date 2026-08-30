import {
  canonicalStringify,
  taggedBlake3Text,
  type CanonicalJson,
} from "@worldstream/pack-sdk";

import type {
  AgreementSignature,
  ExactA202Object,
  FixtureSignature,
  Role,
} from "./model.js";
import { reject } from "./model.js";

export function signerId(role: Role): string {
  switch (role) {
    case "buyer_agent":
      return "agent:northstar:buyer";
    case "seller_agent":
      return "agent:delta:seller";
    case "buyer_approver":
      return "principal:northstar:procurement_director";
    case "venue_signer":
      return "agent:worldstream:venue_signer";
  }
}

export function validateCanonicalText(value: string): void {
  let parsed: CanonicalJson;
  try {
    parsed = JSON.parse(value) as CanonicalJson;
  } catch {
    reject("non_canonical_a202_bytes", "exact A202 bytes are not valid JSON");
  }
  let encoded: string;
  try {
    encoded = canonicalStringify(parsed);
  } catch {
    reject("non_canonical_a202_bytes", "exact A202 bytes are outside canonical JSON");
  }
  if (encoded !== value) {
    reject("non_canonical_a202_bytes", "exact A202 bytes are not canonical");
  }
}

export function validateExactObject(
  object: ExactA202Object,
  expectedSigner: Role,
  expectedPurpose: string,
): void {
  if (new TextEncoder().encode(object.canonical_json).length > 256 * 1024) {
    reject("resource_limit", "A202 object exceeds the revision-bound byte limit");
  }
  validateCanonicalText(object.canonical_json);
  if (taggedBlake3Text(object.canonical_json) !== object.wire_digest) {
    reject("byte_mutation", "A202 exact-byte digest does not match");
  }
  if (object.signature.signed_wire_digest !== object.wire_digest) {
    reject("byte_mutation", "signature does not bind the exact A202 bytes");
  }
  validateFixtureSignature(object.signature, expectedSigner, expectedPurpose);
}

export function validateFixtureSignature(
  signature: FixtureSignature,
  expectedSigner: Role,
  expectedPurpose: string,
): void {
  if (
    signature.signer_role !== expectedSigner ||
    signature.signer_id !== signerId(expectedSigner)
  ) {
    reject("wrong_signer", "protocol signer does not match the required Role identity");
  }
  if (
    signature.purpose !== expectedPurpose ||
    signature.proof !==
      fixtureProof(
        expectedSigner,
        signature.signer_id,
        expectedPurpose,
        signature.signed_wire_digest,
      )
  ) {
    reject("invalid_signature", "fixture signature purpose or proof is invalid");
  }
}

export function validateAgreementProof(signature: AgreementSignature): void {
  if (
    signature.proof !==
    taggedBlake3Text(
      `worldstream/negotiate-agreement-proof/v1\0${signature.signer_role}\0${signature.signer_id}\0${signature.agreement_wire_digest}`,
    )
  ) {
    reject("invalid_signature", "agreement signature proof is invalid");
  }
}

function fixtureProof(
  role: Role,
  signer: string,
  purpose: string,
  wireDigest: string,
): string {
  return taggedBlake3Text(
    `worldstream/negotiate-fixture-proof/v1\0${role}\0${signer}\0${purpose}\0${wireDigest}`,
  );
}
