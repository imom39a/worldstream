# @worldstream/pack-sdk

Deterministic TypeScript primitives and the code-first Activity Pack contract.
This package does not expose filesystem, network, clock, process, or entropy
capabilities. `@worldstream/pack-cli` is the supported compiler and bundler.

## Bounded authoring forms

Descriptors keep the original string form for simple Packs and add explicit
bounded forms where the Host needs a contract. A Role may be a string or
`{ role, minimum, maximum }`. An Action or Event may be a string or
`{ actionType | eventType, payloadSchema }`. Schemas use the closed canonical
subset: `type`, `const`, `enum`, integer and collection bounds, object
properties/required/additionalProperties, items, and description.

`configurationSchema`, `stateSchema`, `projectionSchemas`,
`observationSchemas`, and `externalInputSchemas` declare the remaining Pack
values. Projection and observation maps can name `public`, `participant`,
`operator`, all three historical classes, and `final_reveal`. A view offer may
remain an Action string or declare `{ actionType, eligibilityWindow }`, where
the window has inclusive `opensAt` and exclusive `deadline` timestamps.
