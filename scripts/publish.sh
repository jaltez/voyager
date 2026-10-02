#!/bin/sh
# Publish every workspace crate to crates.io in dependency order.
# cargo publish waits for each crate to become resolvable before the
# dependent one uploads. Bump [workspace.package] version first.
set -eu

for p in vygr-core vygr-providers vygr-llm vygr-research vygr; do
    echo "==> publishing $p"
    cargo publish -p "$p"
done
echo "all crates published"
