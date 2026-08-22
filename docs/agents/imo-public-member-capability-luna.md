# Public member-capability provisioning

The public `POST /v1/operator/member-capabilities` path now provisions the
requested Room member's operational Principal before registering its
Room-member Capability. This closes the live Heist defect where a newly
created Agent seat existed in the Room trace but had no authority Principal
row.

The Principal provisioning change is HostOperator-authorized and uses the
canonical Principal ULID as its stable `AuthorityChangeId`; the existing
request `idempotency_key` remains the Capability change ID. A retry of the
Principal change is accepted only as the durable existing-principal
conflict/no-op branch (SQLite can surface an already-applied replay as its
closed invalid-change result); the subsequent Core authority validation still
enforces Principal kind, enabled status, and exact Room/member/principal
binding. Capability retries remain conflicts because the bearer is one-time
and is never reconstructed or returned again.

Focused coverage includes:

- new Agent Principal plus Room-member Capability;
- existing Principal provisioning;
- duplicate retry and changed-request conflict;
- wrong-principal rejection before mutation;
- one-time bearer and Debug/error-body redaction.

No bearer is included in authority changes, logs, Debug output, or conflict
responses. The HostOperator bearer remains the only credential accepted for
this provisioning operation.
