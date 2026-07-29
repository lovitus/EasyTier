# Punch storm guard probe

This standalone probe compares a no-state burst-boundary loop with the proposed
task-local punch storm guard. It checks:

- fixed state size;
- zero allocations after task state construction;
- constant-time admission and result accounting;
- representative interleaving across 1,024 concurrent peer tasks.

Run it on the configured remote builder, not on the maintainer workstation:

```bash
timeout 600 cargo run --locked --manifest-path \
  tools/punch-storm-guard-probe/Cargo.toml
```

The probe is a production-experiment gate, not a substitute for focused tests
or exact-artifact real-network validation.
