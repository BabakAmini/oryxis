//! Deciding whether a command is safe to run unattended.
//!
//! Two layers, cheapest first: local heuristics that refuse outright (an
//! obviously destructive command, shell chaining that hides one), and
//! only then the model-backed judge with the prompt below. The local
//! pass exists so a judge that is wrong, slow or unreachable is never
//! the only thing between the user and `rm -rf`.

