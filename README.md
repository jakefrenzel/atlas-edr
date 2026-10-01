# Atlas EDR

A home-built Endpoint Detection & Response system: a Rust agent on each Windows endpoint, a Rust server, and a web console.

**Status: early.** The event schema and CI are done, and the ETW sensor is being planned. The design: the agent
collects telemetry through ETW (and later a kernel driver), normalizes it into an OCSF-modeled event schema, runs
Sigma-based detections locally, and ships events to the server over mTLS gRPC. Detection, response and prevention are
built in that order.

## Where things are

- [Architecture overview](docs/architecture-overview.md): goals, stack, principles, **roadmap and current status**, and the decision log.
- [Specs](docs/specs/): one design spec per sub-project (plus brainstorm notes).
- [Plans](docs/plans/): implementation plans.
- [Runbooks](docs/runbooks/): operating procedures (for example, the Hyper-V test VM).
- `crates/`: Rust workspace (`atlas-proto`, `atlas-schema` so far).
- `infra/`: test-VM scripts.

Kernel driver code only ever runs in the Hyper-V test VM, never on a host.

## License

Copyright (C) 2026 jakefrenzel.

Atlas is licensed under the GNU Affero General Public License v3.0 only (AGPL-3.0-only). See [LICENSE](LICENSE).
Commercial licensing may be available separately; contact the author.
