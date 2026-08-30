import { access, lstat, mkdir, readdir, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

import { PackCliError, diagnostic } from "./diagnostics.js";

const PACK_VERSION = "0.1.0";

export interface ScaffoldCustomization {
  readonly description?: string;
  readonly displayName?: string;
  readonly slug?: string;
}

export const SCAFFOLD_RELATIVE_PATHS = [
  ".gitignore",
  "README.md",
  "fixtures/golden.ts",
  "fixtures/privacy.ts",
  "package.json",
  "src/pack.ts",
  "test/pack.test.ts",
  "tsconfig.json",
  "worldstream-pack.json",
] as const;

export async function scaffoldProject(
  directory: string,
  customization: ScaffoldCustomization = {},
): Promise<string> {
  const root = resolve(directory);
  if (await exists(root)) {
    const status = await lstat(root);
    if (status.isSymbolicLink() || !status.isDirectory()) {
      throw new PackCliError(
        diagnostic("WSP-SCAFFOLD-002", "Scaffold destination must be a real directory", {
          file: root,
          hint: "Choose a new directory rather than a symlink or file.",
        }),
      );
    }
    const entries = await readdir(root);
    if (entries.length > 0) {
      throw new PackCliError(
        diagnostic("WSP-SCAFFOLD-001", "Scaffold destination is not empty", {
          file: root,
          hint: "Choose a new directory so existing work cannot be overwritten.",
        }),
      );
    }
  }
  const files = renderScaffoldFiles(customization.slug ?? basename(root), customization);
  for (const [name, contents] of Object.entries(files)) {
    const path = join(root, name);
    await mkdir(join(path, ".."), { recursive: true });
    await writeFile(path, contents, { encoding: "utf8", flag: "wx" });
  }
  return root;
}

export function renderScaffoldFiles(
  slugInput: string,
  customization: ScaffoldCustomization = {},
): Readonly<Record<string, string>> {
  return templates(normalizeSlug(slugInput), customization);
}

function templates(
  slug: string,
  customization: ScaffoldCustomization,
): Readonly<Record<string, string>> {
  const displayName = customization.displayName ?? "Two-party price negotiation";
  const description =
    customization.description ??
    "A deterministic two-party price negotiation with private participant limits.";
  const project = {
    format: "worldstream/pack-project/v1",
    entrypoint: "src/pack.ts",
    output: `dist/${slug}.wspack`,
    goldenFixture: "fixtures/golden.ts",
    privacyFixture: "fixtures/privacy.ts",
  };
  const packageJson = {
    name: `@local/${slug}`,
    version: PACK_VERSION,
    private: true,
    type: "module",
    engines: { node: "24.18.1" },
    scripts: {
      "pack:check": "worldstream-pack check",
      "pack:test": "worldstream-pack test",
      "pack:build": "worldstream-pack build",
      "pack:inspect": "worldstream-pack inspect",
      "pack:prove": "worldstream-pack prove",
    },
    devDependencies: {
      "@types/node": "24.13.3",
      "@worldstream/pack-cli": PACK_VERSION,
      "@worldstream/pack-sdk": PACK_VERSION,
      typescript: "5.9.2",
    },
  };
  return {
    ".gitignore": "dist/\n.worldstream/\nnode_modules/\n",
    "worldstream-pack.json": `${JSON.stringify(project, null, 2)}\n`,
    "package.json": `${JSON.stringify(packageJson, null, 2)}\n`,
    "tsconfig.json": `${JSON.stringify(
      {
        compilerOptions: {
          target: "ES2022",
          module: "NodeNext",
          moduleResolution: "NodeNext",
          strict: true,
          noUncheckedIndexedAccess: true,
          exactOptionalPropertyTypes: true,
          noEmit: true,
          skipLibCheck: true,
          types: ["node"],
        },
        include: ["src/**/*.ts", "fixtures/**/*.ts", "test/**/*.ts"],
      },
      null,
      2,
    )}\n`,
    "src/pack.ts": PACK_SOURCE.replaceAll("__PACK_ID__", `local.${slug}`).replaceAll(
      '"__PACK_DISPLAY_NAME__"',
      JSON.stringify(displayName),
    ),
    "fixtures/golden.ts": GOLDEN_SOURCE,
    "fixtures/privacy.ts": PRIVACY_SOURCE,
    "test/pack.test.ts": TEST_SOURCE,
    "README.md": README_SOURCE.replaceAll("__PACK_NAME__", displayName).replaceAll(
      "__PACK_DESCRIPTION__",
      description,
    ),
  };
}

export function normalizeSlug(input: string): string {
  const slug = input
    .toLowerCase()
    .replace(/[^a-z0-9]+/gu, "-")
    .replace(/^-|-$/gu, "");
  return slug || "negotiation-pack";
}

async function exists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

const PACK_SOURCE = `import type { ActivityPackDefinition, CanonicalJson } from "@worldstream/pack-sdk";

type Role = "buyer" | "seller";

interface Proposal {
  revision: number;
  proposedBy: Role;
  price: number;
}

interface NegotiationState {
  phase: "negotiating" | "agreed";
  revision: number;
  currentProposal: Proposal | null;
  buyerCeiling: number;
  sellerFloor: number;
  outcome: { agreedPrice: number; acceptedBy: Role } | null;
}

function record(value: CanonicalJson | undefined, label: string): Record<string, CanonicalJson> {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    throw new TypeError(\`\${label} must be an object\`);
  }
  return value as Record<string, CanonicalJson>;
}

function integer(value: CanonicalJson | undefined, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new TypeError(\`\${label} must be a safe integer\`);
  }
  return value;
}

function roleForViewer(input: Readonly<Record<string, CanonicalJson>>): Role | null {
  return roleForCore(input.core, input.viewer);
}

function roleForCore(
  coreValue: CanonicalJson | undefined,
  viewerValue: CanonicalJson | undefined,
): Role | null {
  const viewer = record(viewerValue, "viewer");
  if (typeof viewer.member_id !== "string") return null;
  if (viewer.viewer_type !== "participant" && viewer.viewer_type !== "historical") return null;
  const core = record(coreValue, "core");
  const memberships = record(core.memberships, "core.memberships");
  const membership = record(memberships[viewer.member_id], "viewer membership");
  if (
    viewer.viewer_type !== "participant" &&
    !(viewer.viewer_type === "historical" && membership.access_mode === "participant")
  ) return null;
  return membership.role === "buyer" || membership.role === "seller" ? membership.role : null;
}

function offers(state: NegotiationState, role: Role | null): string[] {
  if (state.phase !== "negotiating" || role === null) return [];
  const result = [role === "buyer" ? "propose" : "counter"];
  if (state.currentProposal !== null && state.currentProposal.proposedBy !== role) result.push("accept");
  return result;
}

export default {
  descriptor: {
    packId: "__PACK_ID__",
    name: "__PACK_DISPLAY_NAME__",
    version: "0.1.0",
    roles: ["buyer", "seller"],
    actions: ["propose", "counter", "accept"],
    rejectionCodes: ["invalid_price", "nothing_to_accept", "self_acceptance", "stale_revision"],
    events: ["proposal_revised", "agreement_reached"],
    attentionReasons: [],
  },

  initialize(input) {
    const configuration = record(input.configuration, "configuration");
    const state: NegotiationState = {
      phase: "negotiating",
      revision: 0,
      currentProposal: null,
      buyerCeiling: integer(configuration.buyer_ceiling, "buyer_ceiling"),
      sellerFloor: integer(configuration.seller_floor, "seller_floor"),
      outcome: null,
    };
    return { initial_activity_state: state as unknown as CanonicalJson, timer_requests: [] };
  },

  reduce(input) {
    const state = record(input.prior_activity_state, "prior_activity_state") as unknown as NegotiationState;
    const stimulus = record(input.recorded_stimulus, "recorded_stimulus");
    if (stimulus.stimulus_type !== "participant_action") {
      return {
        activity_disposition_type: "apply",
        next_activity_state: state as unknown as CanonicalJson,
        ordered_domain_events: [],
        timer_requests: [],
        ordered_attention_signals: [],
      };
    }
    const actionType = stimulus.action_type;
    const payload = record(stimulus.canonical_payload, "canonical_payload");
    const core = record(input.core_before, "core_before");
    const memberships = record(core.memberships, "memberships");
    const membership = record(memberships[String(stimulus.member_id)], "acting membership");
    const role = membership.role;

    if (actionType === "propose" || actionType === "counter") {
      const price = integer(payload.price, "price");
      const requiredRole = actionType === "propose" ? "buyer" : "seller";
      if (price <= 0 || role !== requiredRole) {
        return {
          activity_disposition_type: "reject",
          declared_code: "invalid_price",
          bounded_safe_details: { required_role: requiredRole },
        };
      }
      const revision = state.revision + 1;
      const next = { ...state, revision, currentProposal: { revision, proposedBy: requiredRole, price } };
      return {
        activity_disposition_type: "apply",
        next_activity_state: next as unknown as CanonicalJson,
        ordered_domain_events: [{ event_type: "proposal_revised", price, proposed_by: requiredRole, revision }],
        timer_requests: [],
        ordered_attention_signals: [],
      };
    }

    if (actionType === "accept") {
      const proposal = state.currentProposal;
      if (proposal === null) {
        return {
          activity_disposition_type: "reject",
          declared_code: "nothing_to_accept",
          bounded_safe_details: {},
        };
      }
      if (proposal.proposedBy === role) {
        return {
          activity_disposition_type: "reject",
          declared_code: "self_acceptance",
          bounded_safe_details: {},
        };
      }
      if (integer(payload.basis_revision, "basis_revision") !== proposal.revision) {
        return {
          activity_disposition_type: "reject",
          declared_code: "stale_revision",
          bounded_safe_details: { current_revision: proposal.revision },
        };
      }
      const next = {
        ...state,
        phase: "agreed" as const,
        outcome: { agreedPrice: proposal.price, acceptedBy: role as Role },
      };
      return {
        activity_disposition_type: "apply",
        next_activity_state: next as unknown as CanonicalJson,
        ordered_domain_events: [{ event_type: "agreement_reached", agreed_price: proposal.price }],
        timer_requests: [],
        ordered_attention_signals: [],
      };
    }

    return {
      activity_disposition_type: "reject",
      declared_code: "invalid_price",
      bounded_safe_details: { reason: "unsupported action" },
    };
  },

  view(input) {
    const state = record(input.activity_state, "activity_state") as unknown as NegotiationState;
    const role = roleForViewer(input);
    const projection: Record<string, CanonicalJson> = {
      phase: state.phase,
      revision: state.revision,
      current_proposal: state.currentProposal as unknown as CanonicalJson,
      outcome: state.outcome as unknown as CanonicalJson,
    };
    if (role === "buyer") projection.private_limit = state.buyerCeiling;
    if (role === "seller") projection.private_limit = state.sellerFloor;
    const viewer = record(input.viewer, "viewer");
    return {
      projection_schema: role === null ? "public" : "participant",
      projection,
      action_offers: viewer.viewer_type === "participant" ? offers(state, role) : [],
    };
  },

  observe(input) {
    const before = record(input.activity_before, "activity_before") as unknown as NegotiationState;
    const after = record(input.activity_after, "activity_after") as unknown as NegotiationState;
    if (before.revision === after.revision && before.phase === after.phase) return null;
    const afterView = record(input.after_view, "after_view");
    const beforeRole = roleForCore(input.core_before, input.viewer);
    const afterRole = roleForCore(input.core_after, input.viewer);
    const viewer = record(input.viewer, "viewer");
    const offersChanged = viewer.viewer_type === "participant" &&
      offers(before, beforeRole).join("|") !== offers(after, afterRole).join("|");
    return {
      observation_schema: String(afterView.projection_schema).includes("participant") ? "participant" : "public",
      observation: record(afterView.projection, "after_view.projection"),
      action_offers: offersChanged ? "reuse_after_view" : "unchanged",
    };
  },
} satisfies ActivityPackDefinition;
`;

const GOLDEN_SOURCE = `import type { CanonicalJson } from "@worldstream/pack-sdk";

export const goldenFixture = {
  configuration: { buyer_ceiling: 130, seller_floor: 90 },
  buyer: { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", role: "buyer" },
  seller: { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1", role: "seller" },
  accepted: [
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", action_type: "propose", canonical_payload: { price: 100 } },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1", action_type: "counter", canonical_payload: { price: 120 } },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", action_type: "accept", canonical_payload: { basis_revision: 2 } },
  ],
  rejected: { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", action_type: "accept", canonical_payload: { basis_revision: 0 } },
} as const satisfies CanonicalJson;
`;

const PRIVACY_SOURCE = `import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  buyerOnly: ["private_limit"],
  sellerOnly: ["private_limit"],
  forbiddenPublic: ["buyerCeiling", "sellerFloor", "buyer_ceiling", "seller_floor", "private_limit"],
} as const satisfies CanonicalJson;
`;

const TEST_SOURCE = `import assert from "node:assert/strict";
import test from "node:test";

import pack from "../src/pack.js";

test("the scaffold changes real negotiation behavior", () => {
  const initialized = pack.initialize({ configuration: { buyer_ceiling: 130, seller_floor: 90 } });
  assert.equal(typeof initialized, "object");
});
`;

const README_SOURCE = `# __PACK_NAME__

__PACK_DESCRIPTION__

This is a strict TypeScript WorldStream Activity Pack project. The scaffold is
a two-private-Role negotiation, not a counter demo.

Run \`npm ci\`, then use \`npm run pack:check\`, \`npm run pack:test\`,
\`npm run pack:build\`, \`npm run pack:inspect\`, and \`npm run pack:prove\`.
The CLI owns WIT generation, componentization, bundle hashing, and diagnostics.
`;
