---
status: accepted
date: 2026-09-15
---

# Use a terminal interface for Agent Swarm

Agent Swarm's first interactive interface is a TUI on Windows and macOS,
replacing the earlier localhost-browser plan. The user chose a dense terminal
dashboard reference with compact status panels, a work table, live activity,
and visible keyboard shortcuts. This consolidates the interface around the
terminal where the provider CLIs already run, at the cost of terminal-specific
layout, input, rendering, and accessibility constraints.

The TUI presents authorized Room facts and separate Runner activity through
local contracts. Durable coordination and owned agent execution remain outside
the TUI's lifetime: detaching the interface leaves work running, while explicit
Pause and Stop retain the semantics of ADR 0041. TUI exit, terminal closure,
and loss of the client connection must not implicitly abandon a Swarm.

The first TUI shows supervised agent activity and outputs; Directions and
Suggestions enter through Agent Swarm. Embedded interactive provider terminals
are deferred, and provider login uses the provider's normal CLI flow.

This decision selects the application surface. It does not select a TUI
framework. Native Windows
and macOS process, terminal, permission, and subscription-CLI testing remain
necessary; a shared terminal interface does not eliminate platform differences.
