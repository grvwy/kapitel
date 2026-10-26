#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
project_name="kapitel-test-$$"
cleanup() {
  docker compose -p "$project_name" -f compose.test.yaml down --volumes --remove-orphans
}
trap cleanup EXIT INT TERM
docker compose -p "$project_name" -f compose.test.yaml up --build -d kapitel
docker compose -p "$project_name" -f compose.test.yaml run --build --no-deps --rm browser-tests
docker compose -p "$project_name" -f compose.test.yaml run --no-deps --rm benchmark
resident_memory_kib=$(docker compose -p "$project_name" -f compose.test.yaml exec -T kapitel cat /proc/1/status | awk '/^VmRSS:/ {print $2}')
printf 'Resident memory with 1,000 indexed books: %s KiB\n' "$resident_memory_kib"
test "$resident_memory_kib" -lt 131072
