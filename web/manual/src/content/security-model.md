# Security and authority model

WorldStream assumes participants, Runners, artifacts, networks, and even trusted
code can be buggy or hostile. Authority is narrow, purpose-bound, immediately
revocable, and revalidated at the durable mutation boundary.

## Separate authority planes

| Authority | Permits | Does not permit |
| --- | --- | --- |
| Participant Action | exact Room/Membership Action purpose | Runner control, host administration |
| Runner control | claim/renew/complete Activation for assigned agent scope | domain Action submission |
| Host operator | bounded installation/Room/storage operations | automatic Membership in a Room |
| Operator Membership | scoped Room diagnostic view | host control or participant Action |

The assignment-bound MCP helper may internally hold participant and Runner
authority for one assignment, but it exposes only a generic typed tool contract.
Neither bearer, raw routing identity, nor daemon storage becomes model context.

## Capability rules

- Bearers use one canonical wire representation and are always redacted from
  diagnostics.
- Scope, target, purpose, generation, expiry, and revocation are checked before
  mutation and again at writer time where races matter.
- Request and operation identities are bounded and retained for idempotent
  resolution.
- Credentials belong in owner-only files, inherited handles, or the Supervisor
  vault—not command-line arguments, URLs, logs, or browser payloads.

## Projection privacy

Activity Packs create views rather than returning authoritative state and asking
the host to redact it later. A participant receives only its own authorized
Projection and Action Offers. Observation Frames are materialized per
Membership. Visibility loss creates explicit reset/stream consequences; it does
not silently leak old private data.

Agent Heist tests enforce that sealed commitments, fixture truth, and private
exchange details remain absent from public, operator, and unrelated participant
views until the terminal reveal rules permit disclosure.

## Prompt injection boundary

Room text, artifact text, and other participants' content are untrusted input to
an agent policy. WorldStream limits what an Invocation can see and what exact
Actions it can submit, but it does not make an LLM immune to prompt injection.
Agent implementations should:

1. treat Invocation Context as untrusted data, not privileged instruction;
2. select only exact listed Action Offers;
3. validate proposed payloads locally before submission;
4. keep provider credentials and external tools outside Room content;
5. avoid granting external tools broader authority than the assignment.

## Local development rules

- Bind the Runtime, Controller, Activity Client Host, model adapters, and Runner health endpoints
  to loopback.
- Keep `.worldstream/` owner-only and out of source control.
- The authority bootstrap secret binds authority identity; it does **not**
  encrypt SQLite data. Use an encrypted host volume/disk when data-at-rest
  encryption is required.
- Use distinct kind-bound secret references for host, participant, Runner, and
  model-provider material.
- Do not expose the development Supervisor publicly; its trust model is the
  local host operator.
- Run the tracked-file secret scan in the repository gates before pushing.

Primary source: [security model](https://github.com/imom39a/worldstream/blob/main/docs/security.md),
[separate authority ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0003-separate-activation-and-action-authority.md),
and [observation/activation contract](https://github.com/imom39a/worldstream/blob/main/docs/observation-and-activation.md).
