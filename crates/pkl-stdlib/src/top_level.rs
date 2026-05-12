//! Top-level functions / constructors that are always in scope in a Pkl file.
//!
//! Pkl exposes a small set of names that look like function calls but
//! actually construct collection literals: `List(1, 2, 3)`, `Set("a", "b")`,
//! `Map("k", "v")`. These are modeled here so unqualified references
//! resolve.

use crate::StdlibFunction;

static LIST_CTOR: StdlibFunction = StdlibFunction {
    name: "List",
    module: "pkl.base",
    signature: "List<T>(elements: T...): List<T>",
    doc: "Construct a [`List`] from the given elements.",
};

static SET_CTOR: StdlibFunction = StdlibFunction {
    name: "Set",
    module: "pkl.base",
    signature: "Set<T>(elements: T...): Set<T>",
    doc: "Construct a [`Set`] from the given elements.",
};

static MAP_CTOR: StdlibFunction = StdlibFunction {
    name: "Map",
    module: "pkl.base",
    signature: "Map<K, V>(entries: Any...): Map<K, V>",
    doc: "Construct a [`Map`] from alternating key/value arguments.",
};

static PAIR_CTOR: StdlibFunction = StdlibFunction {
    name: "Pair",
    module: "pkl.base",
    signature: "Pair<A, B>(first: A, second: B): Pair<A, B>",
    doc: "Construct a [`Pair`] from `first` and `second`.",
};

static REGEX_CTOR: StdlibFunction = StdlibFunction {
    name: "Regex",
    module: "pkl.base",
    signature: "Regex(pattern: String): Regex",
    doc: "Compile a regular expression.",
};

pub static ALL: &[&StdlibFunction] = &[&LIST_CTOR, &SET_CTOR, &MAP_CTOR, &PAIR_CTOR, &REGEX_CTOR];
