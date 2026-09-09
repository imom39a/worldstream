export const PROJECT_FORMAT_ID = "worldstream/pack-project/v1" as const;
export const BUNDLE_FORMAT_ID = "worldstream/activity-pack-bundle/v1" as const;
export const HOST_CONTRACT_ID = "worldstream/activity-pack/v1" as const;
export const OPERATION_CODEC_ID = "worldstream/activity-pack-operation-codec/v1" as const;
export const CANONICAL_CODEC_ID = "worldstream/canonical-json/v1" as const;
export const EXECUTION_PROFILE_ID = "worldstream/component-deterministic/v1" as const;
export const REVISION_LOCK_ID = "worldstream/pack-revision-lock/v1" as const;

export const GENERATED_REJECTION_SCHEMA = { type: "object" } as const;

export const GENERATED_PACK_LIMITS = {
  maximumAttentionSignals: 16,
  maximumCollectionItems: 256,
  maximumEvents: 32,
  maximumNesting: 32,
  maximumObservationBytes: 65_536,
  maximumProjectionBytes: 65_536,
  maximumStateBytes: 262_144,
  maximumTextBytes: 16_384,
  maximumTimerRequests: 16,
} as const;

export const REQUIRED_COMPONENT_EXPORTS = [
  "descriptor",
  "initialize",
  "reduce",
  "view",
  "observe",
] as const;

export const REQUIRED_BUNDLE_MEMBERS = [
  "revision-lock.json",
  "descriptor.json",
  "schemas.json",
  "codec-bundle.json",
  "executor.component.wasm",
  "golden-corpus.json",
  "conformance.json",
  "dependency-lock.json",
] as const;

export const TOOLCHAIN_VERSIONS = {
  typescript: "5.9.2",
  jco: "1.32.1",
  componentizeJs: "0.22.0",
  nobleHashes: "2.4.0",
} as const;
