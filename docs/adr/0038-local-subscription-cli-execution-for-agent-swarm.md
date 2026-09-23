---
status: accepted
date: 2026-09-15
---

# Use local subscription CLI execution for Agent Swarm

Agent Swarm will use WorldStream as its local coordination core and execute
agents through provider-supplied local CLIs using the user's own subscriptions
and provider-managed sign-in. The user chooses the provider roster. Direct model
API integration and automatic API-key fallback are outside roster agent
execution. [ADR 0043](0043-use-jev-as-an-application-layer-swarm-advisor.md)
adds the sole exception: an optional application-layer JEV advisor with no
Participant or work authority. This reuses the installed agents' execution
capabilities and subscription access, at the cost of provider-specific
adapters, version compatibility, and shared subscription limits.

Each roster member has a human-selected model and an explicit reasoning-effort
setting where the provider exposes that control. Agent Swarm records and
presents these choices and must not silently substitute a model, effort, or
provider default. Unsupported controls are identified as unavailable rather
than assigned an invented equivalent. Work reassignment uses the receiving
roster member's configured choices; it does not silently reconfigure the
unavailable member. Provider-reported resolution and the adapter's ability to
enforce the choices require qualification before claiming strict support.

Prefer explicit versioned provider model identifiers. A moving alias is allowed
only when the human deliberately selects it with its behavior disclosed.
Retain requested and provider-reported resolved identities where available;
detected changes or substitutions block affected execution until resolved.
Auto-routing is not a default, and unreported resolution is not verified
resolution.

Human changes to a member's model or effort apply to its next Invocation and
are recorded while preserving Participant identity. The current turn finishes
unless the human explicitly requests interruption. Where supported, each
member reuses an explicitly identified private provider conversation within
its Swarm, with fresh authorized Room context on each turn. An unavailable
conversation is replaced using a recorded handoff and the same selected
settings; provider-private memory does not override Room authority.

WorldStream, coordination storage, the application UI, and Runner processes
reside on the user's computer; provider CLIs may contact their model services.
No externally deployed application infrastructure is required. Model execution
remains outside the Room Runtime, preserving ADR 0001's authority separation.
This decision establishes the requested application constraints; it does not
qualify a provider adapter, settle the Swarm work model or steering semantics,
or select a desktop framework.
