# Rust Workspace

This directory contains the Rust implementation of the multi-agent runtime.

## Crates

### `multi-agent-protocol`
Shared provider-neutral objects:
- `WorkspaceSpec`
- `RoleSpec`
- `TaskDispatch`
- `WorkspaceEvent`

### `multi-agent-runtime-core`
Core runtime state machine:
- queue dispatch
- start dispatch
- progress dispatch
- complete dispatch
- attach final result text

### `multi-agent-runtime-cteno`
Trait-based Cteno embedding layer:
- workspace bootstrap
- role agent provisioning
- role session spawning
- session message delivery

This crate is intentionally decoupled from the private Cteno codebase.
It defines the adapter seam now, so the real Cteno integration can implement these traits later.

## Run Tests

```bash
cd rust
cargo test
```
