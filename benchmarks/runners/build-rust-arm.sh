#!/bin/sh
set -eu

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <arm-name> <binary-name> [cargo build arguments...]" >&2
  exit 2
fi

arm=$1
binary=$2
shift 2
cache_root=${REPOTRACER_BENCH_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/repotracer/benchmarks}
target_dir=${CARGO_TARGET_DIR:-$cache_root/target}
bin_dir=${REPOTRACER_BENCH_BIN_DIR:-$cache_root/bin}
profile=${REPOTRACER_BENCH_PROFILE:-debug}

mkdir -p "$target_dir" "$bin_dir"
CARGO_TARGET_DIR=$target_dir cargo build "$@"
cp "$target_dir/$profile/$binary" "$bin_dir/$arm-$binary"
printf '%s\n' "$bin_dir/$arm-$binary"
