# Glossary

| Term | Meaning |
| --- | --- |
| Room | one authoritative shared situation governed by one Activity Pack |
| Activity Pack | deterministic reusable Room rules |
| Activity Pack Revision | immutable executable semantic identity pinned by a Room |
| Activity Pack Bundle | immutable `.wspack` bytes carrying one portable revision and its proof material |
| Activity Client | independently executing application using a scoped WorldStream client contract; it may be Pack-aware but owns no Room authority |
| Activity Client Release | immutable content-addressed client build with exact artifacts and Client Surfaces |
| Client Surface | one human-facing entry point of an Activity Client Release |
| Activity Distribution | integrator manifest referencing separate exact Pack and client releases without granting approval |
| Client Deployment | Host-approved operational availability of one exact client Release at exact launch targets |
| Deployment Trust Level | Host classification of a Deployment as verified from exact bytes or externally trusted |
| Client Binding | Host-local exact Pack/contract/Access Mode/Role association to an approved Client Surface |
| Client Binding Store | Host-owned operational Releases, Deployments, Bindings, and fallback; never Room or Replay state |
| Client Selection | resolution of current Membership and Host bindings to one approved Surface for one handoff |
| WorldStream Inspector | Pack-neutral authorized protocol workbench and explicitly configured fallback Activity Client |
| Activity Phase | pack-defined stage, separate from Room status |
| Outcome | pack-defined final result, separate from archive |
| Principal | durable human or agent identity |
| Membership | one Principal's Room-local seat |
| Participant | human or agent Room Member with participant access and a Role; Membership standing separately controls current Action authority |
| Role | pack-defined responsibility |
| Action | typed participant proposal |
| Action Offer | exact current Action allowed from one authorized view |
| Stimulus | recorded candidate input to Room reduction |
| Transition | one ordered accepted authoritative change |
| Semantic Time | recorded stimulus-specific effective time |
| Projection | complete current Membership-authorized view |
| Observation Frame | durable Membership-addressed consequence of a Transition |
| Cursor | acknowledged position in one Membership's Observation Stream |
| Projection Reset | complete authorized stream baseline |
| Catch-up | retained Observation delivery after a Cursor |
| Replay | read-only reconstruction of Canonical History |
| Runner | external host authorized to start agent Invocations |
| Invocation | one bounded execution of an agent policy |
| Attention Signal | pack indication that an agent may need to act |
| Activation Intent | durable request for a Runner to consider an Invocation |
| Invocation Context | bounded authorized information for one Invocation |
| Agent-Private Memory | agent-owned state outside WorldStream |
| Room Head | exact sequence and hash lineage identifying current committed truth |
| Room Integrity State | operational healthy/faulted/quarantined assessment |
| Semantic Receipt | durable stable result for one operation identity |
| Runner Template | owner-installed immutable executable/health/capacity manifest |
| Agent Profile | immutable assignment/host/provider configuration revision |
| Pack Author | person or organization defining an Activity Pack; authorship grants no install or Room authority |
| Application Integrator | person or organization connecting a Pack, clients, Runners, protocol adapters, and deployment |

Use the full [domain context](https://github.com/imom39a/worldstream/blob/main/CONTEXT.md)
for canonical definitions and explicit “avoid” guidance.
