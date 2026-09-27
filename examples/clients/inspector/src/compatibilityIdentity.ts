import identitySource from "../public/compatibility-identity.json?raw";

export const CLIENT_CONTRACT_IDENTITY_JSON = identitySource;
export const CLIENT_CONTRACT_IDENTITY: {
  schema: string;
  manifest_schema: string;
  manifest_revision: number;
  product: string;
  wire: string;
  config: number;
  storage_schema: number;
  core_schema_version: string;
  hash_suite: string;
  pack_executors: Array<Record<string, string | boolean>>;
} = JSON.parse(identitySource);

/** The manifest-derived identity embedded beside the built UI assets. */
export const CLIENT_CONTRACT_IDENTITY_PATH = "/compatibility-identity.json";
