# Notes presentation

Owns tree rows, drag previews, menus and destination projections, Markdown editing transformations, editor body, toolbar and conflict controls. Desktop retains the note catalog, move validation, selection, input and document entities, title, dirty state, save generation, 800ms debounce, conflict resolution and close coordination. Components dispatch typed actions through weak-owner callbacks and do not persist documents.

Dependencies are enforced by `scripts/ci/check_architecture.py`, including development and target-specific dependencies. GPUI components use `nyaterm-ui`; direct `gpui-kit`, desktop and store dependencies are forbidden.
