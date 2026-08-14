# ADR 0002: Sequence Domain-Relevant Room Changes

Status: Accepted, 2026-08-14

WorldStream records every change that can alter Authoritative Room State—including accepted participant Actions, timers, external inputs, Membership or Role changes, and Room archival—as a Stimulus and Transition in the same Room order. Session presence, Runner availability, lease operations, telemetry, and other operational facts do not enter that order. This makes Replay reconstruct the complete Room and gives every durable Observation an honest causal position, at the cost of serializing administrative changes with participant activity.
