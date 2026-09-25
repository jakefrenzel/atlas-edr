# EDR Project

A home-built Endpoint Detection & Response system (agent + server), Windows first.

**Read `docs/architecture-overview.md` first** — it holds the goals, stack, architectural principles, roadmap of sub-projects, open decisions, and decision log. Detailed per-sub-project designs live in `docs/specs/`, implementation plans in `docs/plans/`, runbooks in `docs/runbooks/`.

**Resuming work:** check the roadmap's status column. A sub-project marked "In brainstorm" links to a `*-brainstorm-notes.md` handoff in `docs/specs/`; resume from its "Next steps" section.

## Working agreements

- Take it slow; optimize for the best approach, not the familiar one.
- Every sub-project goes through: brainstorm → written spec (reviewed) → implementation plan (reviewed) → build. No code before the spec and plan are approved.
- When a decision is made, add it to the Decision Log in `docs/architecture-overview.md` and update the roadmap status.
- Kernel driver code is only ever loaded in the Hyper-V test VM, never on the host.
