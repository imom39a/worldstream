# Reviewed Room setup inputs

These reusable JSON inputs use the [Room Setup Specification editor schema](../../docs/schemas/room-setup-v1.schema.json).
They are input examples, not installed approvals or retained setup operations.
The CLI backend is being delivered behind `cli-operator-preview`; an example
file does not claim that the complete create/connect/launch flow is released.

- Agent Heist 0.2.0 uses its existing lobby, one human Navigator, one external
  Agent Insider, and an optional unfilled Broker. Its configuration supplies the
  required Role choice; generic resolution fills the exact schema's constants.
- Negotiate 0.1.0 uses three external Agents and one human Buyer Approver.
  It starts at creation, not through a generic pause or lobby. Its
  `formation_deadline` is explicit Unix seconds: `4102444800` means
  2100-01-01 00:00:00 UTC. Review and change this absolute deadline before a
  real negotiation; it is not a duration or an automatically expanded value.

Principal references are specification-local labels for new per-attempt
identities, not existing Principal lookups. Creation must retain the resolved
identities for retry. These external assignments need separately delivered
Runner and Membership credentials; these files contain neither credential.
External Agents do not require a local Agent Profile. A managed Agent must
explicitly select both an installed Agent Profile revision and an approved,
compatible Runner Template revision; neither is inferred or installed by setup.
They do not start external processes, install Packs, approve Runner templates,
or authorize browser Client Deployments. Operator Membership is off by default.
An explicit `operator_view: true` asks creation for a separate roleless,
read-only Operator Membership. Every registered v1 Pack must declare Operator
Projection and Observation schemas; this does not grant participant-private
visibility or substitute Host control authority for Membership credentials.

Both inputs follow the same generic schema resolver. Negotiate's retained
configuration schema declares only an object; schema validation cannot certify
its business rules. Its reviewed configuration comes from the reference Pack's
fixture, with an explicitly future deadline. The exact Pack remains responsible
for semantic acceptance at creation. No immutable Pack schema is rewritten.

## Configuration resolution

Resolution never expands environment variables or evaluates expressions. Omitted
properties receive declared `const` values first, then declared `default` values;
an explicitly supplied value is never replaced. Fixed-value conflicts, missing
required values, and undeclared fields under `additionalProperties: false` are
errors. Unknown input field names and supplied values are not echoed in errors.

The supported schema subset is explicit `type` (object, array, string, integer,
number, boolean, or null), `const`, `default`, `enum`, `properties`, `required`,
boolean `additionalProperties`, one `items` schema, `minItems`, `maxItems`,
`minimum`, `maximum`, `minLength`, and `maxLength`. `$schema`, `title`, and
`description` are annotations. Other keywords, including composition and
references, fail closed rather than being silently ignored. String length bounds
count Unicode characters, not encoded bytes.

Inputs are limited to 1 MiB, 4,096 JSON values, 32 levels of nesting, and 64 seats.
Duplicate JSON keys are rejected. Secrets, credentials, generated authority,
executable settings, approvals, and retained operation state do not belong in
this specification. Server TOML and prerequisite import declarations remain
separate inputs.

## Preview CLI flow

Use a `worldstreamctl` build with `cli-operator-preview` enabled and an already
running managed local Controller/Runtime with these exact Pack revisions installed
and selectable. These commands do not start services or install missing Packs.
For a non-default installation, pass the same `--state-dir` and `--controller`
options used to start it.

```sh
worldstreamctl room example --pack worldstream.agent-heist@0.2.0 --output heist-setup.json
worldstreamctl room validate --file heist-setup.json

worldstreamctl room example --pack worldstream.negotiate@0.1.0 --output negotiate-setup.json
worldstreamctl room validate --file negotiate-setup.json
```

Review and edit the generated input before creating a Room. `example` resolves
the installed name/version to its exact digest, applies the same generic resolver
as `validate`, and refuses to overwrite an existing output file. `validate` reads
catalog and explicitly selected dependency metadata; it creates no Room or
credentials. Add `--json` for a structured result with bounded field-path errors.

This MVP generates only the two reviewed exact revisions above. Other revisions
or missing choices return an explicit error without creating output. Interactive
prompts are deferred: `--interactive` is not supported by this preview. A local
write failure can leave an incomplete output file; inspect it before retrying
with a new output path.
