# File browser presentation

Owns path/breadcrumb projection (including raw remote paths), table cells, transfer queue rendering, file preview bodies, tabs and toolbar and the document editor surface. Desktop retains the file-manager workspace, panes, input/document entities, navigation, listing caches, transfer targets, permissions and all I/O. Queue and editor callbacks carry intents back to the existing controllers; selections and raw path tokens remain captured by desktop before deferred operations.

Dependencies are enforced by `scripts/ci/check_architecture.py`, including development and target-specific dependencies. GPUI components use `nyaterm-ui`; direct `gpui-kit`, desktop and store dependencies are forbidden.
