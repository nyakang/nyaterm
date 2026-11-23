# Stats, GPU, NPU, Processes and Docker panels

The panel owns its presentation geometry, scroll positions and demand clock. Desktop pushes immutable snapshots and input handles; it owns data, derived-query caches, SSH leases, refresh jobs, settings and confirmation dialogs. Actions are typed. Host tokens are validated against the current session and concrete SSH lease in desktop, including before a deferred snapshot flush.

Handlers capture a weak application entity and defer snapshot delivery until the panel borrow ends. Poll ticks execute outside the panel borrow. The default-off `test-support` feature provides integration observations and fixtures.

Dependencies are enforced by `scripts/ci/check_architecture.py`, including development and target-specific dependencies. GPUI components use `nyaterm-ui`; direct `gpui-kit`, desktop and store dependencies are forbidden.
