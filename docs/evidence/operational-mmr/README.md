# Operational MMR local qualification

These reports exercise the storage-neutral MMR primitive with the exact
length-prefixed Frame leaf used by the adapters. Each tier builds the complete
immutable node inventory, samples 1,000 evenly distributed leaves, fetches only
the Core-produced sibling/peak coordinates, verifies against the final root,
and confirms that changing the leaf bytes fails verification.

The million-leaf tier retained 1,999,993 immutable nodes but needed at most 25
nodes for a proof. All four tiers verified exactly and rejected tampering. This
is local performance evidence; the pinned Linux/Fly image reproduces the same
four measurements alongside the million-transition provider qualification.
