# CodeG patch to kill_tree 0.2.4

Source: https://github.com/oneofthezombies/kill-tree , commit
4e99191f9f3dcbc832794405855e23ef7d194b72 , crates/libs/kill_tree.
MIT, copyright 2024 oneofthezombies. Upstream LICENSE retained verbatim.

The upstream breadth-first traversal does not track visited PIDs. Cyclic OS
parent-PID snapshots cause unbounded allocation. This patch rejects repeated
PIDs and any traversal into the current process before executing any kill.
It does not skip duplicate edges and then kill an ambiguous process set.
Valid-tree ordering and include_target behavior remain unchanged.

A synthetic cycle reproduces the independent defect without enumerating or
terminating real processes. The published fix2 Server allocation failure is
not proven to originate here. This patch is not evidence of that attribution.

The existing killer implementations retain their concrete owned return type.
A local lint allowance documents that intentional trait refinement; using an
opaque type here would capture the delegated builder's short-lived borrow.

The existing error-display test compares against the actual standard-library
source diagnostic, whose wording changed in Rust 1.99, instead of hardcoding
an older Rust message. Production error handling is unchanged.

Vendored source/docs use normalized LF and trailing whitespace; LICENSE is unchanged.
