# Changelog

All notable changes to this project will be documented in this file.

## 0.2.0 - 2026-03-23

### Changed
- Moved supervisor state ownership to a dedicated actor and removed cross-task locking.
- Unified the project naming around `rvisor` across the binary, docs, and service integration.
- Refined supervisor internals after the actor migration to keep the command and IPC surface stable.

### Added
- Added Apache-2.0 licensing metadata and repository license text.
