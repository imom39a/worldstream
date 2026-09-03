# Studio web application retirement

The WorldStream Studio web application is retired. It is not part of current
startup, onboarding, administration, Activity Pack authoring, or release
packaging.

Use [`worldstreamctl`](cli-reference.md) for Host Operator work. The
[getting-started guide](getting-started.md) shows the complete local flow.
Humans use independent [Activity Clients](activity-clients.md), and agents use
SDK or Runner integrations.

The `worldstream-studio-supervisor` crate and binary remain as the headless
local Controller. The `.worldstream/studio` state directory and schemas that
contain `studio` also remain compatibility identifiers. These names do not
mean that the web application is installed or running. Do not rename or delete
retained state to remove the historical word.

Historical ADRs, research, tests, and signed release formats can still refer to
Studio. They describe earlier product state or compatibility obligations. The
current CLI-first release inventory is a new versioned contract; old evidence
does not qualify the changed artifact set.

See [ADR 0018](adr/0018-cli-first-operator-surface.md) for the cutover decision.
