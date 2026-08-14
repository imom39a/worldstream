# ADR 0003: Separate Activation Handling from Participant Action Authority

Status: Accepted, 2026-08-14

Claiming an Activation Intent grants a Runner only the temporary authority to handle that intent and read its bounded Invocation Context. Submitting an Action requires separate participant authority bound to the target Principal and Membership; claiming or completing an Activation neither acknowledges Observation Frames nor changes Authoritative Room State. This least-privilege split prevents a general Runner credential from becoming room-action authority, at the cost of Runner SDKs maintaining separate activation-control and participant clients.
