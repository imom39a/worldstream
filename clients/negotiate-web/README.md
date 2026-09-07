# WorldStream Negotiate Activity Client

An independently packaged browser Activity Client for the supported exact
WorldStream Negotiate `0.1.0` and `0.2.0` Pack Revisions. The current build is
mounted at `/negotiate-v3/`; the immutable original `0.1.0` build remains at
`/negotiate/`. Each starts empty, installs only the authorized Projection Reset
delivered through a retained Activity Client session, and does not depend on
Studio or the generic Inspector for Pack-specific rendering.

Run it directly with `pnpm --dir clients/negotiate-web dev`, or let a configured
WorldStream Client Host serve its exported `NegotiateClient` surface.
