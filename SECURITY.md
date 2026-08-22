# WorldStream security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through
[GitHub Security Advisories](https://github.com/imom39a/worldstream/security/advisories/new).
Do not open a public issue or discussion for a vulnerability.

Include the affected version or commit, the smallest safe reproduction, the
expected impact, and any mitigations you have already tried. Do not include
real credentials, participant-private observations, prompts, model output,
artifact contents, or other private proof data. Use synthetic or redacted data
and offer to coordinate a safer transfer if additional evidence is necessary.

We aim to acknowledge a report within three business days, provide an initial
assessment within seven business days, and send an update at least every
fourteen calendar days until resolution. These are response targets rather
than a guarantee. Please allow time for a coordinated fix and disclosure before
publishing details.

## Supported versions

| Version | Security updates |
| --- | --- |
| Latest published `0.1.x` release | Supported |
| Unreleased default branch | Best effort; not a supported deployment |
| Older releases and source snapshots | Not supported |

WorldStream has not yet published its first supported release. Until that
release exists, reports against the default branch are welcome but no source
snapshot should be treated as production-supported software.

## Disclosure and scope

We will validate the report, assess affected versions, prepare tests and a fix,
and coordinate release and disclosure timing with the reporter. We may request
additional synthetic evidence. Reports about exposed secrets or live user data
should contain only enough redacted metadata to locate the incident safely.

The current support and deployment boundaries are documented in
[`docs/architecture.md`](docs/architecture.md) and the reviewed
[`compatibility.toml`](compatibility.toml) manifest. A documented unsupported
configuration can still reveal a security defect; please report it privately
when exploitation could cross a supported trust boundary or affect another
user.
