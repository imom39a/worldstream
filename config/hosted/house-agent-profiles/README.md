# Midnight Archive House Agent Profile sources

`mira-1.json` and `jonah-1.json` are the exact reviewed publish inputs for the
two named Midnight Archive companions. Each source selects the narrow
`managed_house_openrouter` host contract and the Archive-only
`openrouter-house-archive` Runner Template revision `1`. The sources contain no
credential or secret value. `hosted-openrouter` is only the name of the Host's
separately imported credential record.

The canonical publish-input digests are
`blake3:074c3df5883f252ad0ce8c8db3cba2d2ed31b44a5dd265b2e67000b359a2a9fd`
for Mira and
`blake3:ee202951744bb38bc0cbf5262db24a4d8762badd4ed8c8379e45d0b6c60fd730`
for Jonah. These identify the reviewed inputs. The installed Agent Profile
digests are different because the Host record contains its opaque, local
credential reference; the private approval receipt records those installed
digests.

`scripts/hosted-runtime.mjs` writes the same source objects into the production
and local initialization review. It renders the Runner Template only after the
packaged `worldstream-managed-agent-host` bytes have been verified and retained
under their BLAKE3 digest. This permits the Archive and retained Heist templates
to use the same executable bytes while keeping their immutable Pack
compatibility and instance identities separate.

The Runner Template's `instances` and `health` fields are required by the
current shared Runner Template schema. For a `managed_house_openrouter` Profile,
`hosted-archive-house-01` is a dormant reference identity used to select the
exact template and retain that identity through Task Setup and the House runtime
binding. Its `127.0.0.1:9608` address is not the readiness endpoint for a House
Runner unit and these sources make no such claim. After Genesis, the Controller
revalidates the exact Profile, template identity, retained executable digest,
and run-scoped binding before `ManagedAgentHostOperations` starts the
`worldstream-assignment-mcp` and `worldstream-managed-agent-host` child pair.
That pair becomes ready only after the model host parses its private startup
frame, opens the allowance ledger, and completes the MCP initialization
exchange.

The manifest's `maximum_concurrent_invocations: 4` is an instance-level schema
ceiling. It does not grant four units per template. The Controller's retained
House reservation coordinator and the platform's single `runner_unit` capacity
gate independently cap the whole deployment at four active House Runner units,
across all House revisions and template identities, with at most two in one
Run.

Initialization review and import publish Host records but do not approve a
House Agent Revision. The installation-specific operator approval recipe in
`scripts/hosted-house-approval.mjs` must bind the canonical House revision,
published Profile, installed Runner Template, retained executable, and named
credential. Availability also requires the separately checked exact provider
route. No paid or live-provider qualification is claimed by these files.

A follow-up should add an executable-only managed House template form so this
approval anchor no longer needs dormant instance and health fields. That change
must preserve exact executable/Pack compatibility checks, immutable template
identity, Task Setup selection, retained runtime binding, and the existing
managed child-pair readiness contract.
