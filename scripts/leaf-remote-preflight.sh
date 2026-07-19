#!/usr/bin/env bash

set -euo pipefail

readonly BUILDER_HOST="${BUILDER_HOST:-root@192.168.2.160}"
readonly BUILDER_CONTAINER="${BUILDER_CONTAINER:-easytier-debug-builder}"
readonly REMOTE_HOST_WORKSPACE="${REMOTE_HOST_WORKSPACE:-/data/easytier-builder/workspace}"
readonly REMOTE_WORKSPACE="${REMOTE_WORKSPACE:-/workspace}"
readonly BUILD_TIMEOUT="${BUILD_TIMEOUT:-1800}"
readonly TEST_TIMEOUT="${TEST_TIMEOUT:-300}"
readonly MINIMAL_BUILD_LOG="/tmp/easytier_leaf_preflight_policy_minimal.log"
readonly MUSL_MINIMAL_BUILD_LOG="/tmp/easytier_leaf_preflight_policy_musl_minimal.log"
readonly BUILD_LOG="/tmp/easytier_leaf_preflight_build.log"
readonly TEST_LOG="/tmp/easytier_leaf_preflight_test.log"

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

readonly -a SSH_OPTIONS=(
  -o ServerAliveInterval=30
  -o ServerAliveCountMax=3
  -o ConnectTimeout=10
)
readonly -a BUILD_SSH_OPTIONS=(
  "${SSH_OPTIONS[@]}"
  -o ExitOnForwardFailure=yes
  -R 7890:127.0.0.1:7890
)

readonly -a DEFAULT_EASYTIER_TEST_FILTERS=(
  common::underlay_guard::tests::should_block_configured_and_runtime_addresses_when_enabled
  core::tests::check_config_fully_parses_policy_only_input_like_mihomo_test_mode
  gateway::socks5::dataplane::tests::mesh_only_connect_never_falls_back_to_kernel
  peers::peer_ospf_route::tests::peer_removal_restarts_remaining_generation_and_invalidates_remote_cache
  policy_proxy::mesh_udp_relay::tests
  instance::instance::tests::socks_egress_guard_shutdown_waits_for_owned_task
  launcher::tests::network_config_roundtrips_experimental_features
)
readonly -a DEFAULT_POLICY_TEST_FILTERS=(
  config::tests::parses_legacy_and_named_udp_modes_canonically
  config::tests::validates_shadowsocks_fields_without_expanding_mesh_semantics
  config::tests::validates_layered_protocol_fields_without_expanding_mesh_semantics
  config::tests::rejects_uot_v2_on_socks5
  leaf_config::tests::compiles_shadowsocks_native_udp_and_uot_chain_as_leaf_actors
  leaf_config::tests::compiles_trojan_vmess_and_vless_as_private_transport_chains
  leaf_config::tests::compiles_stable_yaml_to_strict_leaf_config
  config::tests::validates_custom_ipv4_fake_dns_range
  config::tests::validates_custom_ipv6_fake_dns_range
  leaf_config::tests::explicit_dns_sets_replace_platform_direct_and_keep_proxy_separate
  leaf_config::tests::expands_system_dns_to_captured_platform_servers_for_proxy_bootstrap
  leaf_config::tests::preserves_unresolved_domain_contract_for_direct_socks_and_fallback
  packet::unix_bridge::tests::preserves_boundaries_in_both_directions
  packet::unix_bridge::tests::unsupported_packet_batch_request_keeps_legacy_backend
  packet::unix_bridge::tests::memory_batch_bridge_preserves_order_and_boundaries
  packet::unix_bridge::tests::stream_batch_bridge_preserves_order_and_detects_close
  packet::unix_bridge::tests::contiguous_batch_body_rejects_corrupt_lengths
  packet::unix_bridge::tests::stream_endpoint_adapter_preserves_leaf_channel_ownership
)
readonly -a DEFAULT_NETSTACK_TEST_FILTERS=(
  stack::tests::full_ingress_channel_wakes_waiting_stack_sender
  device::tests::full_output_preserves_ingress_until_capacity_returns
  device::tests::bounded_ingress_backpressures_and_preserves_order
  device::tests::capacity_wait_makes_progress_with_queued_output_sender
  device::tests::unused_reserved_output_capacity_is_released_after_poll
  tcp::tests::runner_exits_when_output_receiver_is_dropped
  tcp::tests::immediate_poll_path_keeps_runtime_cooperative
)

usage() {
  cat <<'EOF'
Usage: scripts/leaf-remote-preflight.sh [ADDITIONAL_TEST_FILTER ...]

Synchronizes the complete local snapshot to the dedicated builder, checks the
policy crate without default features, performs the locked debug no-run build
for the EasyTier Leaf/HEV library target, and executes the focused tests directly
from that exact test binary. Additional filters are appended to the default
candidate suite.

Environment overrides:
  BUILDER_HOST, BUILDER_CONTAINER, REMOTE_HOST_WORKSPACE, REMOTE_WORKSPACE
  BUILD_TIMEOUT, TEST_TIMEOUT
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

sync_snapshot() {
  printf 'Syncing complete candidate snapshot to %s:%s\n' \
    "$BUILDER_HOST" "$REMOTE_HOST_WORKSPACE"
  rsync -a --delete \
    --exclude '/.git/' \
    --exclude '/target/' \
    --exclude '/.artifacts/' \
    --exclude '/.claude/' \
    --exclude '/.envrc.local' \
    --exclude '/easytier-gui/src-tauri/gen/' \
    --exclude '/easytier-gui/src-tauri/.gradle/' \
    -e "ssh ${SSH_OPTIONS[*]}" \
    "$REPOSITORY_ROOT/" "$BUILDER_HOST:$REMOTE_HOST_WORKSPACE/"
}

check_builder_idle() {
  local status
  status="$({
    ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
      "docker exec $BUILDER_CONTAINER bash -c 'if pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; then pgrep -a -x cargo; pgrep -a -x rustc; echo BLOCKED; else echo CLEAR; fi'"
  } 2>&1)"
  printf '%s\n' "$status"
  if [[ "$status" != *CLEAR* || "$status" == *BLOCKED* ]]; then
    printf 'Remote builder is busy; inspect the reported process before retrying.\n' >&2
    exit 2
  fi
}

run_no_run_build() {
  local exit_code=0
  ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'cd $REMOTE_WORKSPACE && CARGO_BUILD_JOBS=\$(nproc) CARGO_PROFILE_TEST_OPT_LEVEL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=1 timeout $BUILD_TIMEOUT cargo test --locked --no-run --package easytier --package easytier-policy --package netstack-smoltcp --lib --features easytier/leaf-policy-proxy,easytier-policy/leaf-inprocess > $BUILD_LOG 2>&1; code=\$?; echo EXIT_CODE=\$code; exit \$code'" \
    || exit_code=$?

  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER tail -50 $BUILD_LOG"
  if ((exit_code != 0)); then
    printf 'Remote no-run build failed with exit code %d.\n' "$exit_code" >&2
    exit "$exit_code"
  fi
}

run_policy_minimal_no_run_build() {
  local exit_code=0
  ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'cd $REMOTE_WORKSPACE && CARGO_BUILD_JOBS=\$(nproc) CARGO_PROFILE_TEST_OPT_LEVEL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=1 timeout $BUILD_TIMEOUT cargo test --locked --no-run --package easytier-policy --lib --no-default-features > $MINIMAL_BUILD_LOG 2>&1; code=\$?; echo EXIT_CODE=\$code; exit \$code'" \
    || exit_code=$?

  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER tail -50 $MINIMAL_BUILD_LOG"
  if ((exit_code != 0)); then
    printf 'Remote policy no-default no-run build failed with exit code %d.\n' "$exit_code" >&2
    exit "$exit_code"
  fi
}

run_policy_musl_minimal_no_run_build() {
  local exit_code=0
  ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'export BINDGEN_EXTRA_CLANG_ARGS=\"-I/usr/include/x86_64-linux-musl\"; export CC_x86_64_unknown_linux_musl=musl-gcc; export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc; cd $REMOTE_WORKSPACE && CARGO_BUILD_JOBS=\$(nproc) CARGO_PROFILE_TEST_OPT_LEVEL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=1 timeout $BUILD_TIMEOUT cargo test --locked --no-run --target x86_64-unknown-linux-musl --package easytier-policy --lib --no-default-features > $MUSL_MINIMAL_BUILD_LOG 2>&1; code=\$?; echo EXIT_CODE=\$code; exit \$code'" \
    || exit_code=$?

  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER tail -50 $MUSL_MINIMAL_BUILD_LOG"
  if ((exit_code != 0)); then
    printf 'Remote policy musl no-default no-run build failed with exit code %d.\n' "$exit_code" >&2
    exit "$exit_code"
  fi
}

resolve_test_binary() {
  local binary_prefix="$1"
  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c \"awk '/Executable unittests src\\/lib.rs/ && /\\/deps\\/$binary_prefix-/ { line=\\\$0 } END { if (line == \\\"\\\") exit 1; sub(/^.*\\\\(/, \\\"\\\", line); sub(/\\\\).*$/, \\\"\\\", line); print \\\"$REMOTE_WORKSPACE/\\\" line }' $BUILD_LOG\""
}

run_focused_tests() {
  local test_binary="$1"
  local log_mode="$2"
  shift 2
  local -a filters=("$@")
  local remote_script

  if [[ "$log_mode" == "reset" ]]; then
    printf -v remote_script 'set -euo pipefail; : > %q;' "$TEST_LOG"
  else
    printf -v remote_script 'set -euo pipefail; : >> %q;' "$TEST_LOG"
  fi
  local filter
  for filter in "${filters[@]}"; do
    printf -v remote_script '%s match_output="$(timeout %q %q %q --list)"; if [[ "$match_output" != *": test"* ]]; then printf %q %q >> %q; exit 97; fi; printf %q %q >> %q; timeout %q %q %q --nocapture --test-threads 1 >> %q 2>&1;' \
      "$remote_script" "$TEST_TIMEOUT" "$test_binary" "$filter" \
      '%s\n' "ERROR: no test matched $filter" "$TEST_LOG" \
      '%s\n' "=== TEST $filter ===" "$TEST_LOG" \
      "$TEST_TIMEOUT" "$test_binary" "$filter" "$TEST_LOG"
  done

  local exit_code=0
  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c $(printf '%q' "$remote_script")" \
    || exit_code=$?
  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER tail -100 $TEST_LOG"
  if ((exit_code != 0)); then
    printf 'Focused test suite failed with exit code %d.\n' "$exit_code" >&2
    exit "$exit_code"
  fi
}

sync_snapshot
check_builder_idle
run_policy_minimal_no_run_build
run_policy_musl_minimal_no_run_build
run_no_run_build
readonly EASYTIER_TEST_BINARY="$(resolve_test_binary easytier)"
readonly POLICY_TEST_BINARY="$(resolve_test_binary easytier_policy)"
readonly NETSTACK_TEST_BINARY="$(resolve_test_binary netstack_smoltcp)"
printf 'Using exact EasyTier library test binary: %s\n' "$EASYTIER_TEST_BINARY"
printf 'Using exact policy library test binary: %s\n' "$POLICY_TEST_BINARY"
printf 'Using exact netstack library test binary: %s\n' "$NETSTACK_TEST_BINARY"
run_focused_tests "$EASYTIER_TEST_BINARY" reset "${DEFAULT_EASYTIER_TEST_FILTERS[@]}" "$@"
run_focused_tests "$NETSTACK_TEST_BINARY" append "${DEFAULT_NETSTACK_TEST_FILTERS[@]}"
run_focused_tests "$POLICY_TEST_BINARY" append "${DEFAULT_POLICY_TEST_FILTERS[@]}"
printf 'Leaf/HEV remote preflight passed. GitHub release artifacts were not built.\n'
