import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { finalizeArtifacts, generateSemanticArtifacts } from "./artifacts.js";
import { retainedTranscriptDigest } from "./conformance.js";
import { loadProject } from "./project.js";
import { WorldStreamPackToolchain } from "./toolchain.js";
import { PackCliError } from "./diagnostics.js";
import { runBehavioralConformance } from "./conformance.js";

test("fresh scaffold passes strict check and behavioral conformance", async () => {
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "test-"));
  const root = join(parent, "vendor-negotiation");
  const toolchain = new WorldStreamPackToolchain();
  const created = await toolchain.new(root);
  assert.equal(created.status, "created");
  const checked = await toolchain.check(root);
  assert.equal(checked.status, "passed");
  const tested = await toolchain.test(root);
  assert.equal(tested.acceptedActions, 3);
  assert.equal(tested.declaredRejections, 1);
  assert.equal(tested.privateViewsDistinct, true);
});

test("check returns owned capability and module-state diagnostics", async () => {
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "diagnostic-"));
  const root = join(parent, "unsafe-pack");
  const toolchain = new WorldStreamPackToolchain();
  await toolchain.new(root);
  await writeFile(
    join(root, "src", "ambient.ts"),
    "export let callbackCount = 0;\nexport function now(): number { return Date.now(); }\n",
  );
  await assert.rejects(
    () => toolchain.check(root),
    (error: unknown) => {
      assert.ok(error instanceof PackCliError);
      const codes = new Set(error.diagnostics.map((item) => item.code));
      assert.ok(codes.has("WSP-DETERMINISM-001"));
      assert.ok(codes.has("WSP-CAPABILITY-002"));
      return true;
    },
  );
});

test("retained golden identity accepts only a Core-authored tagged digest", () => {
  const local = `blake3:${"a".repeat(64)}`;
  const authoritative = `blake3:${"b".repeat(64)}`;
  assert.equal(retainedTranscriptDigest({}, local), local);
  assert.equal(
    retainedTranscriptDigest({ expected_transcript_digest: authoritative }, local),
    authoritative,
  );
  assert.throws(
    () => retainedTranscriptDigest({ expected_transcript_digest: "b".repeat(64) }, local),
    (error: unknown) =>
      error instanceof PackCliError &&
      error.diagnostics.some((item) => item.code === "WSP-GOLDEN-004"),
  );
});

test("archive fixture proves bounded roles, schemas, inputs, and all viewer classes", async () => {
  const root = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const toolchain = new WorldStreamPackToolchain();
  const checked = await toolchain.check(root);
  const tested = await toolchain.test(root);
  const built = await toolchain.build(root);
  const inspected = await toolchain.inspect(built.bundle);
  const evidence = await runBehavioralConformance(await loadProject(root));
  const semantic = generateSemanticArtifacts(evidence);

  assert.equal(checked.status, "passed");
  assert.equal(tested.acceptedActions, 3);
  assert.equal(tested.declaredRejections, 1);
  assert.equal(evidence.roles.find((role) => role.role === "mira")?.minimum, 0);
  assert.equal(evidence.roles.find((role) => role.role === "jonah")?.maximum, 1);
  assert.equal(evidence.externalInputs.length, 1);
  assert.equal(evidence.crossRoleMutationHidden, true);
  assert.deepEqual(Object.keys(evidence.projectionSchemas).sort(), [
    "final_reveal",
    "historical_operator",
    "historical_participant",
    "historical_public",
    "operator",
    "participant",
    "public",
  ]);
  assert.ok(semantic.schemaIds["stimulus:archive.fixture/briefing-opened/v1"]);
  assert.ok(inspected.members.includes("golden-corpus.json"));
  assert.equal(built.componentExports.join(","), "descriptor,initialize,reduce,view,observe");

  const finalized = finalizeArtifacts(evidence, semantic, new Uint8Array([1]));
  const corpus = finalized.goldenCorpus as Record<string, unknown>;
  assert.equal(Array.isArray(corpus.external_inputs), true);
  assert.equal((corpus.external_inputs as unknown[]).length, 1);
});

test("archive fixture rejects unsupported schemas and undeclared ExternalInput kinds", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "archive-contract-"));
  const unsupportedSchema = join(parent, "unsupported-schema");
  const undeclaredInput = join(parent, "undeclared-input");
  const unsupportedStart = join(parent, "unsupported-start");
  const invalidStartPayload = join(parent, "invalid-start-payload");
  const oversizedStart = join(parent, "oversized-start");
  const malformedStartRejection = join(parent, "malformed-start-rejection");
  const invalidStartRejectionDetails = join(parent, "invalid-start-rejection-details");
  const oversizedStartRejectionDetails = join(parent, "oversized-start-rejection-details");
  await cp(fixture, unsupportedSchema, { recursive: true });
  await cp(fixture, undeclaredInput, { recursive: true });
  await cp(fixture, unsupportedStart, { recursive: true });
  await cp(fixture, invalidStartPayload, { recursive: true });
  await cp(fixture, oversizedStart, { recursive: true });
  await cp(fixture, malformedStartRejection, { recursive: true });
  await cp(fixture, invalidStartRejectionDetails, { recursive: true });
  await cp(fixture, oversizedStartRejectionDetails, { recursive: true });
  const toolchain = new WorldStreamPackToolchain();

  const schemaPath = join(unsupportedSchema, "src", "pack.ts");
  await writeFile(
    schemaPath,
    (await readFile(schemaPath, "utf8")).replace(
      'payloadSchema: {\n        additionalProperties: false,',
      'payloadSchema: {\n        $ref: "unsupported",\n        additionalProperties: false,',
    ),
  );
  await assert.rejects(() => toolchain.test(unsupportedSchema), PackCliError);

  const goldenPath = join(undeclaredInput, "fixtures", "golden.ts");
  await writeFile(
    goldenPath,
    (await readFile(goldenPath, "utf8")).replace(
      'input_type: "archive.fixture/briefing-opened/v1",',
      'input_type: "archive.fixture/undeclared/v1",',
    ),
  );
  await assert.rejects(() => toolchain.test(undeclaredInput), PackCliError);

  await replaceStartContract(
    unsupportedStart,
    'contract: "worldstream/activity-start/v1"',
    'contract: "worldstream/activity-start/v2"',
  );
  await assert.rejects(() => toolchain.test(unsupportedStart), PackCliError);

  await replaceStartContract(
    invalidStartPayload,
    'canonicalPayload: { opened_by: "host" }',
    'canonicalPayload: { opened_by: "agent" }',
  );
  await assert.rejects(() => toolchain.test(invalidStartPayload), PackCliError);

  await replaceStartContract(
    oversizedStart,
    'preStartPhase: "briefing"',
    `preStartPhase: "${"x".repeat(257)}"`,
  );
  await assert.rejects(() => toolchain.test(oversizedStart), PackCliError);

  const malformedStartSource = join(malformedStartRejection, "src", "pack.ts");
  await writeFile(
    malformedStartSource,
    (await readFile(malformedStartSource, "utf8")).replace(
      'return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "inactive" };',
      'return { activity_disposition_type: "attention", bounded_safe_details: {}, declared_code: "inactive" };',
    ),
  );
  await assert.rejects(() => toolchain.test(malformedStartRejection), PackCliError);

  const invalidDetailsSource = join(invalidStartRejectionDetails, "src", "pack.ts");
  await writeFile(
    invalidDetailsSource,
    (await readFile(invalidDetailsSource, "utf8")).replace(
      'return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "inactive" };',
      'return { activity_disposition_type: "reject", bounded_safe_details: [], declared_code: "inactive" };',
    ),
  );
  await assert.rejects(() => toolchain.test(invalidStartRejectionDetails), PackCliError);

  const oversizedDetailsSource = join(oversizedStartRejectionDetails, "src", "pack.ts");
  await writeFile(
    oversizedDetailsSource,
    (await readFile(oversizedDetailsSource, "utf8")).replace(
      'return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "inactive" };',
      `return { activity_disposition_type: "reject", bounded_safe_details: { reason: "${"x".repeat(16_385)}" }, declared_code: "inactive" };`,
    ),
  );
  await assert.rejects(() => toolchain.test(oversizedStartRejectionDetails), PackCliError);
});

test("archive fixture exercises solo and optional-role rosters through conformance", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "archive-roster-"));
  const toolchain = new WorldStreamPackToolchain();
  const rosterCases = `rosterCases: [
    { participants: [lead] },
    { participants: [lead, mira] },
    { participants: [lead, jonah] },
    { participants: [lead, mira, jonah] },
  ],`;
  const cases = [
    ["solo", "[lead, spectator, operator]", true],
    ["mira", "[lead, mira, spectator, operator]", true],
    ["jonah", "[lead, jonah, spectator, operator]", true],
    ["missing-lead", "[mira, spectator, operator]", false],
    [
      "duplicate-mira",
      "[lead, mira, { ...mira, member_id: \"01ARZ3NDEKTSV4RRFFQ69G5FCA\" }, spectator, operator]",
      false,
    ],
  ] as const;

  for (const [name, participants, valid] of cases) {
    const root = join(parent, name);
    await cp(fixture, root, { recursive: true });
    const goldenPath = join(root, "fixtures", "golden.ts");
    const golden = await readFile(goldenPath, "utf8");
    await writeFile(goldenPath, valid
      ? golden.replace(rosterCases, `rosterCases: [{ participants: ${participants} }],`)
      : golden.replace(
          "participants: [lead, mira, jonah, spectator, operator],",
          `participants: ${participants},`,
        ));
    if (valid) {
      assert.equal((await toolchain.test(root)).status, "passed");
    } else {
      await assert.rejects(() => toolchain.test(root), PackCliError);
    }
  }
});

test("string-declared Actions may emit timed offers before an accepted Action", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "timed-offer-"));
  const root = join(parent, "string-action");
  await cp(fixture, root, { recursive: true });
  const source = join(root, "src", "pack.ts");
  await writeFile(
    source,
    (await readFile(source, "utf8")).replace(
      /actions: \[\{[\s\S]*?\n    \}\],\n    attentionReasons:/u,
      'actions: ["inspect"],\n    attentionReasons:',
    ).replace("liveParticipant && current.phase === \"active\"", "liveParticipant && current.inspections === 0"),
  );
  const golden = join(root, "fixtures", "golden.ts");
  await writeFile(
    golden,
    (await readFile(golden, "utf8")).replace(
      /  invalidActions: \[[\s\S]*?  participants:/u,
      "  invalidActions: [],\n  participants:",
    ),
  );

  const toolchain = new WorldStreamPackToolchain();
  await toolchain.build(root);
  const wrapper = await readFile(join(root, ".worldstream", "generated", "activity-pack-wrapper.ts"), "utf8");

  assert.match(wrapper, /action_offers: unknown\[\]/u);
  assert.match(wrapper, /eligibility_window: actionOffer\.eligibilityWindow/u);

  const legacyRoot = join(parent, "legacy");
  await toolchain.new(legacyRoot);
  await toolchain.build(legacyRoot);
  const legacyWrapper = await readFile(
    join(legacyRoot, ".worldstream", "generated", "activity-pack-wrapper.ts"),
    "utf8",
  );
  assert.match(legacyWrapper, /action_offers: unknown\[\]/u);
});

test("legacy audience aliases compile through the generated Component lookup", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const root = join(await mkdtemp(join(scratch, "legacy-alias-component-")), "pack");
  await cp(fixture, root, { recursive: true });
  const source = join(root, "src", "pack.ts");
  await writeFile(
    source,
    (await readFile(source, "utf8"))
      .replace(/    observationSchemas: \{[\s\S]*?\n    \},\n    packId:/u, "    packId:")
      .replace(/    projectionSchemas: \{[\s\S]*?\n    \},\n    rejectionCodes:/u, "    rejectionCodes:")
      .replace('if (viewer.viewer_type === "operator") return "operator";', 'return "operator";'),
  );
  const toolchain = new WorldStreamPackToolchain();
  await toolchain.build(root);
  const wrapper = await readFile(join(root, ".worldstream", "generated", "activity-pack-wrapper.ts"), "utf8");
  assert.match(wrapper, /projection:operator/u);
  assert.match(wrapper, /observation:operator/u);
});

test("legacy audience aliases use the retained public and participant schema references", async () => {
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const root = join(await mkdtemp(join(scratch, "legacy-audiences-")), "pack");
  const toolchain = new WorldStreamPackToolchain();
  await toolchain.new(root);
  const artifacts = generateSemanticArtifacts(await runBehavioralConformance(await loadProject(root)));
  const descriptor = artifacts.descriptorContent as Record<string, Record<string, Record<string, string>>>;
  const projections = descriptor.projection_schemas!;
  const observations = descriptor.observation_schemas!;
  assert.equal("activity_start_contract" in descriptor, false);

  assert.equal(projections.operator!.schema_id, projections.public!.schema_id);
  assert.equal(projections.historical_public!.schema_id, projections.public!.schema_id);
  assert.equal(projections.historical_participant!.schema_id, projections.participant!.schema_id);
  assert.equal(projections.historical_operator!.schema_id, projections.public!.schema_id);
  assert.equal(projections.final_reveal!.schema_id, projections.public!.schema_id);
  assert.equal(observations.operator!.schema_id, observations.public!.schema_id);
  assert.equal(observations.historical_participant!.schema_id, observations.participant!.schema_id);
  assert.equal(observations.final_reveal!.schema_id, observations.public!.schema_id);
});

test("schema validation mirrors the Core subset shape and UTF-8 string limits", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "schema-parity-"));
  const rejectedSchemas = [
    '{ pattern: "x", type: "string" }',
    "{ type: 4 }",
    '{ type: "integer", minimum: "0" }',
    '{ type: "integer", minimum: 2, maximum: -2 }',
    '{ type: "array", items: { type: "string" }, required: [] }',
    '{ type: "object", properties: { value: { type: "string" } }, required: ["missing"] }',
    '{ type: "object", additionalProperties: "false" }',
    '{ type: "string", minLength: -1 }',
    '{ enum: [] }',
  ];
  const toolchain = new WorldStreamPackToolchain();
  for (const [index, schema] of rejectedSchemas.entries()) {
    const root = join(parent, `invalid-${index}`);
    await cp(fixture, root, { recursive: true });
    await replaceConfigurationSchema(root, schema);
    await assert.rejects(() => toolchain.test(root), PackCliError);
  }

  const acceptedRoot = join(parent, "utf8-accepted");
  await cp(fixture, acceptedRoot, { recursive: true });
  await replaceConfigurationSchema(acceptedRoot, '{ type: "string", maxLength: 2 }');
  await replaceGoldenConfiguration(acceptedRoot, '"é"');
  assert.equal((await toolchain.test(acceptedRoot)).status, "passed");

  const rejectedRoot = join(parent, "utf8-rejected");
  await cp(fixture, rejectedRoot, { recursive: true });
  await replaceConfigurationSchema(rejectedRoot, '{ type: "string", maxLength: 1 }');
  await replaceGoldenConfiguration(rejectedRoot, '"é"');
  await assert.rejects(() => toolchain.test(rejectedRoot), PackCliError);
});

test("privacy mutations must alter state, preserve hidden viewers, and affect an owning viewer", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "privacy-mutation-"));
  const oneHiddenOwnerChanges = join(parent, "one-hidden-owner-changes");
  await cp(fixture, oneHiddenOwnerChanges, { recursive: true });
  const oneHiddenPrivacy = join(oneHiddenOwnerChanges, "fixtures", "privacy.ts");
  await writeFile(
    oneHiddenPrivacy,
    (await readFile(oneHiddenPrivacy, "utf8")).replace(
      'hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FC0", "01ARZ3NDEKTSV4RRFFQ69G5FC1"]',
      'hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FC0"]',
    ),
  );
  const toolchain = new WorldStreamPackToolchain();
  assert.equal((await toolchain.test(oneHiddenOwnerChanges)).status, "passed");
  const mutations = [
    ["unchanged-state", 'jonah_secret: "changed"', 'jonah_secret: "violet"'],
    [
      "no-hidden-viewer",
      'hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FC0", "01ARZ3NDEKTSV4RRFFQ69G5FC1"]',
      "hidden_from: []",
    ],
    ["hidden-view-changes", 'lead_secret: "amber"', 'lead_secret: "changed"'],
  ] as const;
  for (const [name, from, to] of mutations) {
    const root = join(parent, name);
    await cp(fixture, root, { recursive: true });
    const privacy = join(root, "fixtures", "privacy.ts");
    await writeFile(privacy, (await readFile(privacy, "utf8")).replace(from, to));
    await assert.rejects(() => toolchain.test(root), PackCliError);
  }
});

test("conformance assigns production room sequences after successful commits only", async () => {
  const fixture = fileURLToPath(new URL("../fixtures/archive-contract", import.meta.url));
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "room-sequences-"));

  const oneInput = join(parent, "one-input");
  await cp(fixture, oneInput, { recursive: true });
  await recordRoomSequence(oneInput);
  assert.deepEqual(committedSequences(await runBehavioralConformance(await loadProject(oneInput))), [1, 2, 3, 4]);

  const zeroInput = join(parent, "zero-input");
  await cp(fixture, zeroInput, { recursive: true });
  await recordRoomSequence(zeroInput);
  const source = join(zeroInput, "src", "pack.ts");
  await writeFile(
    source,
    (await readFile(source, "utf8"))
      .replace('phase: "briefing",', 'phase: "active",')
      .replace(
        /    activityStartContract: \{[\s\S]*?\n    \},\n/u,
        "",
      )
      .replace(
        'if (current.phase !== "active") {',
        'if (current.phase !== "active" || stimulus.member_id === "01ARZ3NDEKTSV4RRFFQ69G5FC8") {',
      ),
  );
  const golden = join(zeroInput, "fixtures", "golden.ts");
  await writeFile(
    golden,
    (await readFile(golden, "utf8"))
      .replace(/  externalInputs: \[\{[\s\S]*?\n  \}\],\n/u, "")
      .replace(
        /rejected: \{([\s\S]*?)member_id: lead\.member_id,/u,
        "rejected: {$1member_id: spectator.member_id,",
      ),
  );
  assert.deepEqual(committedSequences(await runBehavioralConformance(await loadProject(zeroInput))), [1, 2, 3]);
});

async function replaceConfigurationSchema(root: string, schema: string): Promise<void> {
  const source = join(root, "src", "pack.ts");
  await writeFile(
    source,
    (await readFile(source, "utf8")).replace(
      /configurationSchema: \{[\s\S]*?\n    \},\n    events:/u,
      `configurationSchema: ${schema},\n    events:`,
    ),
  );
}

async function replaceStartContract(root: string, expected: string, replacement: string): Promise<void> {
  const source = join(root, "src", "pack.ts");
  const contents = await readFile(source, "utf8");
  assert.equal(contents.includes(expected), true, `fixture is missing ${expected}`);
  await writeFile(
    source,
    contents.replace(expected, replacement),
  );
}

async function replaceGoldenConfiguration(root: string, configuration: string): Promise<void> {
  const golden = join(root, "fixtures", "golden.ts");
  await writeFile(
    golden,
    (await readFile(golden, "utf8")).replace(
      'configuration: { archive_id: "midnight" },',
      `configuration: ${configuration},`,
    ),
  );
}

async function recordRoomSequence(root: string): Promise<void> {
  const source = join(root, "src", "pack.ts");
  await writeFile(
    source,
    (await readFile(source, "utf8"))
      .replace(
        'inspections: { type: "integer" },',
        'inspections: { type: "integer" },\n      last_sequence: { type: "integer" },',
      )
      .replace(
        'required: ["phase", "inspections", "lead_secret", "mira_secret", "jonah_secret"],',
        'required: ["phase", "inspections", "last_sequence", "lead_secret", "mira_secret", "jonah_secret"],',
      )
      .replace('inspections: 0,', 'inspections: 0,\n        last_sequence: 0,')
      .replace(
        'next_activity_state: { ...current, phase: "active" },',
        'next_activity_state: { ...current, last_sequence: Number(input.next_room_seq), phase: "active" },',
      )
      .replace(
        'next_activity_state: { ...current, inspections },',
        'next_activity_state: { ...current, inspections, last_sequence: Number(input.next_room_seq) },',
      ),
  );
  const privacy = join(root, "fixtures", "privacy.ts");
  await writeFile(
    privacy,
    (await readFile(privacy, "utf8")).replace(
      "inspections: 3,",
      "inspections: 3,\n      last_sequence: 4,",
    ),
  );
}

function committedSequences(evidence: Awaited<ReturnType<typeof runBehavioralConformance>>): number[] {
  return evidence.transcript.flatMap((item) => {
    if (item === null || Array.isArray(item) || typeof item !== "object") return [];
    const state = (item as Record<string, unknown>).next_activity_state;
    if (state === null || Array.isArray(state) || typeof state !== "object") return [];
    const sequence = (state as Record<string, unknown>).last_sequence;
    return typeof sequence === "number" ? [sequence] : [];
  });
}
