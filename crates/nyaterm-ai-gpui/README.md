# AI presentation

Owns transient step/phase contracts, composer geometry and control layout, message bubbles and controls, tool-call frames and approval cards. Inputs and selectable transcript text are supplied by desktop. Desktop retains transcript caches/jobs, native/CLI runtime, conversation history, capability queues and stale-approval validation. Approval callbacks retain the original run, AI session and request identities; the component receives no executable capability arguments or runtime handle.

Dependencies are enforced by `scripts/ci/check_architecture.py`, including development and target-specific dependencies. GPUI components use `nyaterm-ui`; direct `gpui-kit`, desktop and store dependencies are forbidden.
