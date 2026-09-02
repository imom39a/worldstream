# WorldStream Negotiate Activity Client

An independently packaged browser Activity Client for the exact WorldStream
Negotiate `0.1.0` Pack Revision. It starts empty, installs only the authorized
Projection Reset delivered through a retained Activity Client session, and does
not depend on Studio or the generic Inspector for Pack-specific rendering.

Run it directly with `pnpm --dir clients/negotiate-web dev`, or let a configured
WorldStream Client Host serve its exported `NegotiateClient` surface.
