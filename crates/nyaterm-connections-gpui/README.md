# Connections presentation

Owns protocol/section display contracts, the shared connection and batch editor frame, form captions/selects/switches, action buttons, saved-connection rows, detail projection and hover cards. Desktop retains catalog caches, drafts, credential authority, discovery, input entities, validation, persistence and window ownership. The existing narrow editor-frame host routes navigation and save/cancel actions to each controller. Detail/share projections enumerate safe displayed fields instead of serializing saved connections.

Dependencies are enforced by `scripts/ci/check_architecture.py`, including development and target-specific dependencies. GPUI components use `nyaterm-ui`; direct `gpui-kit`, desktop and store dependencies are forbidden.
