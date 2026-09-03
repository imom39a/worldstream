# Managed process implementation notes

Status: IMO-137's MVP implementation and focused checks are complete behind the
internal `cli-operator-preview` feature. These are engineering notes, not a
production qualification or full CLI-only Heist acceptance claim. The approved
public contract remains in [the CLI-first plan](cli-first-implementation-plan.md).

## Ownership and control

The installation has two process roles: Controller and Runtime. Each role has
an owner-protected launch generation and an exclusive OS lifetime lease. A PID
is diagnostic information. It does not authorize a signal or process adoption.

A process claims its generation before it opens mutable application storage.
It holds its lease until shutdown. A clean terminal record and a released
lease are distinct facts. A crash or incomplete record must not be reported as
a successful stop. A delayed child cannot claim a generation that recovery has
already fenced.

An explicit Controller start also holds a separate permanent startup lock until
the child proves readiness. This lock belongs to the launcher, not the child.
It prevents another live CLI invocation from treating a delayed child as an
abandoned launch. An exited launcher releases this OS lock automatically. A
later explicit start must still check the child's lifetime lease and endpoint;
lock release alone is not proof that the child stopped.

Before sending authority, the caller challenges the listener with a fresh
nonce. The proof binds the installation, role, generation, PID, and actual
accepted socket's local address. The caller then sends the request on that
same TCP connection. There is no second connection, redirect, or transparent
retry after proof.

The managed Runtime uses its existing operator listener for proof and control.
A separate private proof listener followed by an unverified Host connection
would leave an address-reuse race. The process shell must pass the actual local
socket address to the proof handler, while preserving the existing peer address
used for admission and rate limits. An HTTP Host header is not socket evidence.

Runtime status/stop authority is derived for that Runtime generation. It is
different from Controller control access, Host authority, Membership authority,
Runner authority, and provider credentials. It must not authorize Host or Room
routes. The unleased foreground Runtime keeps its existing service-manager
operation; it does not become a managed process implicitly.

This protects local endpoint selection. It does not claim protection against
privileged network interception or a process that can read the owner's private
keys.

## Configuration and process boundaries

The CLI resolves the selected configuration before an explicit managed start.
It stores a protected, normalized configuration declaration in Controller state.
This declaration contains secret file references, not resolved secret values.
Both child processes run with fixed executable paths, a fixed working directory,
and detached standard input, output, and error streams. Ambient configuration
overrides do not silently change the child after the CLI has resolved them.

A later read or stop uses retained process evidence. It must not require the
original configuration file to exist. A new start must check the requested
configuration against retained launch facts; it must not silently reconfigure a
running installation. A missing or damaged ownership record is not permission
to recreate a lock or adopt a process found at the configured port.

The existing lifecycle HTTP aliases use the same managed coordinator when the
Controller holds a managed lease. They must not call the older PID-based
foreground lifecycle adapter in that mode. Controller-only stop is a separate
endpoint and does not request Runtime shutdown.

## Restart progress

The Controller retains an operational checkpoint before process mutation. The
checkpoint is not canonical Room state or an Activity Phase.

1. Pause new managed starts and capture only owned running targets.
   In the MVP, reject restart when this set includes a bound Runner Template
   instance. Restore the previous start-admission state before returning. Do not
   stop a process or write an operation checkpoint for this rejection.
2. Stop those targets. Keep an owned handle when shutdown fails.
3. Stop the verified Runtime and verify lease release.
4. Start the configured Runtime and verify readiness.
5. Restore only supported captured targets through the existing launch checks.
   Retain progress after each restored target. Automatic restoration of bound
   Runner Template instances, with current task eligibility checks, is deferred
   to IMO-147.

A retry continues the retained operation and restore set. It must not replace
that set with an empty list just because the earlier attempt stopped the
processes. External and previously inactive agents do not enter the restore
set. A partial result remains partial; it does not imply rollback or restored
model process memory.

The start fence covers both managed Agent Hosts and installed Runner instances.
An explicit installation stop keeps this fence closed. A later successful start
opens the fence, but it does not restart the previously stopped agents. An
unresolved retained running process must block coordinated shutdown instead of
being silently omitted from the captured set.

## Verification scope

Coordinator tests use the real operation stores and registries. Only the OS
process and Runtime control seams are substituted. Separate process tests must
prove detached startup, singleton ownership, terminal closure, controller
failure, bounded logs, and safe recovery before IMO-137 is complete.

All process tests must use disposable state and test-owned processes and ports.
Existing developer services are outside their scope. Local macOS results are
not native Windows qualification or release qualification.

MVP closure checks: end-to-end CLI recovery, bounded lifecycle logs, managed
Host-authority proxy connections using same-socket proof, rejection of unsupported
bound-Runner restart before shutdown, and focused regression tests and review.
Expanded fault-recovery matrices and native-platform qualification are deferred
to IMO-147. These notes do not claim production-grade process supervision.
