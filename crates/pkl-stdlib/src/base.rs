//! Built-in types declared in the `pkl.base` module.
//!
//! Each [`StdlibType`] lists the most common members so hover / completion
//! has something useful to display. Coverage is not exhaustive — uncommon
//! members are added as we run into them. Signatures follow the Pkl style:
//! `name(arg: T): R` for methods, `name: T` for properties.

use crate::{MemberKind, StdlibKind, StdlibMember, StdlibType};

// ----------------------------------------------------------------------
// Top type and bottom type

static ANY: StdlibType = StdlibType {
    name: "Any",
    module: "pkl.base",
    kind: StdlibKind::AbstractClass,
    generics: &[],
    extends: None,
    doc: "Top type — supertype of every Pkl value.",
    members: &[
        StdlibMember {
            name: "getClass",
            kind: MemberKind::Method,
            signature: "getClass(): Class",
            doc: "Returns the runtime class of this value.",
        },
        StdlibMember {
            name: "toString",
            kind: MemberKind::Method,
            signature: "toString(): String",
            doc: "Returns a string representation of this value.",
        },
    ],
};

static NULL_TYPE: StdlibType = StdlibType {
    name: "Null",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Type of the literal `null`.",
    members: &[],
};

// ----------------------------------------------------------------------
// Booleans

static BOOLEAN: StdlibType = StdlibType {
    name: "Boolean",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Truth values: `true` and `false`.",
    members: &[
        StdlibMember {
            name: "xor",
            kind: MemberKind::Method,
            signature: "xor(other: Boolean): Boolean",
            doc: "Exclusive-or with `other`.",
        },
        StdlibMember {
            name: "implies",
            kind: MemberKind::Method,
            signature: "implies(other: Boolean): Boolean",
            doc: "Logical implication: `!this || other`.",
        },
    ],
};

// ----------------------------------------------------------------------
// Numbers

static NUMBER: StdlibType = StdlibType {
    name: "Number",
    module: "pkl.base",
    kind: StdlibKind::AbstractClass,
    generics: &[],
    extends: Some("Any"),
    doc: "Supertype of `Int` and `Float`.",
    members: &[
        StdlibMember {
            name: "abs",
            kind: MemberKind::Property,
            signature: "abs: Number",
            doc: "Absolute value of this number.",
        },
        StdlibMember {
            name: "sign",
            kind: MemberKind::Property,
            signature: "sign: Number",
            doc: "`-1`, `0`, or `1` depending on the sign of this number.",
        },
        StdlibMember {
            name: "isPositive",
            kind: MemberKind::Property,
            signature: "isPositive: Boolean",
            doc: "True if this number is greater than zero.",
        },
        StdlibMember {
            name: "toString",
            kind: MemberKind::Method,
            signature: "toString(): String",
            doc: "Decimal string representation of this number.",
        },
    ],
};

static INT: StdlibType = StdlibType {
    name: "Int",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Number"),
    doc: "64-bit signed integer.",
    members: &[
        StdlibMember {
            name: "inverse",
            kind: MemberKind::Property,
            signature: "inverse: Int",
            doc: "Bitwise negation.",
        },
        StdlibMember {
            name: "isOdd",
            kind: MemberKind::Property,
            signature: "isOdd: Boolean",
            doc: "True if this integer is odd.",
        },
        StdlibMember {
            name: "isEven",
            kind: MemberKind::Property,
            signature: "isEven: Boolean",
            doc: "True if this integer is even.",
        },
        StdlibMember {
            name: "truncatingDivide",
            kind: MemberKind::Method,
            signature: "truncatingDivide(other: Int): Int",
            doc: "Integer division, truncating any fractional part.",
        },
        StdlibMember {
            name: "shl",
            kind: MemberKind::Method,
            signature: "shl(places: Int): Int",
            doc: "Logical left shift.",
        },
        StdlibMember {
            name: "shr",
            kind: MemberKind::Method,
            signature: "shr(places: Int): Int",
            doc: "Arithmetic right shift.",
        },
        StdlibMember {
            name: "ushr",
            kind: MemberKind::Method,
            signature: "ushr(places: Int): Int",
            doc: "Unsigned right shift.",
        },
        StdlibMember {
            name: "and",
            kind: MemberKind::Method,
            signature: "and(other: Int): Int",
            doc: "Bitwise AND.",
        },
        StdlibMember {
            name: "or",
            kind: MemberKind::Method,
            signature: "or(other: Int): Int",
            doc: "Bitwise OR.",
        },
        StdlibMember {
            name: "xor",
            kind: MemberKind::Method,
            signature: "xor(other: Int): Int",
            doc: "Bitwise XOR.",
        },
        // Duration unit suffixes — `5.s`, `200.ms`, etc.
        StdlibMember {
            name: "ns",
            kind: MemberKind::Property,
            signature: "ns: Duration",
            doc: "Construct a [`Duration`] of `this` nanoseconds.",
        },
        StdlibMember {
            name: "us",
            kind: MemberKind::Property,
            signature: "us: Duration",
            doc: "Construct a [`Duration`] of `this` microseconds.",
        },
        StdlibMember {
            name: "ms",
            kind: MemberKind::Property,
            signature: "ms: Duration",
            doc: "Construct a [`Duration`] of `this` milliseconds.",
        },
        StdlibMember {
            name: "s",
            kind: MemberKind::Property,
            signature: "s: Duration",
            doc: "Construct a [`Duration`] of `this` seconds.",
        },
        StdlibMember {
            name: "min",
            kind: MemberKind::Property,
            signature: "min: Duration",
            doc: "Construct a [`Duration`] of `this` minutes.",
        },
        StdlibMember {
            name: "h",
            kind: MemberKind::Property,
            signature: "h: Duration",
            doc: "Construct a [`Duration`] of `this` hours.",
        },
        StdlibMember {
            name: "d",
            kind: MemberKind::Property,
            signature: "d: Duration",
            doc: "Construct a [`Duration`] of `this` days.",
        },
        // DataSize unit suffixes.
        StdlibMember {
            name: "b",
            kind: MemberKind::Property,
            signature: "b: DataSize",
            doc: "Construct a [`DataSize`] of `this` bytes.",
        },
        StdlibMember {
            name: "kb",
            kind: MemberKind::Property,
            signature: "kb: DataSize",
            doc: "Construct a [`DataSize`] of `this` kilobytes (1000 bytes).",
        },
        StdlibMember {
            name: "mb",
            kind: MemberKind::Property,
            signature: "mb: DataSize",
            doc: "Construct a [`DataSize`] of `this` megabytes (1 000 000 bytes).",
        },
        StdlibMember {
            name: "gb",
            kind: MemberKind::Property,
            signature: "gb: DataSize",
            doc: "Construct a [`DataSize`] of `this` gigabytes (10^9 bytes).",
        },
        StdlibMember {
            name: "kib",
            kind: MemberKind::Property,
            signature: "kib: DataSize",
            doc: "Construct a [`DataSize`] of `this` kibibytes (1024 bytes).",
        },
        StdlibMember {
            name: "mib",
            kind: MemberKind::Property,
            signature: "mib: DataSize",
            doc: "Construct a [`DataSize`] of `this` mebibytes (1024^2 bytes).",
        },
        StdlibMember {
            name: "gib",
            kind: MemberKind::Property,
            signature: "gib: DataSize",
            doc: "Construct a [`DataSize`] of `this` gibibytes (1024^3 bytes).",
        },
    ],
};

static FLOAT: StdlibType = StdlibType {
    name: "Float",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Number"),
    doc: "64-bit IEEE 754 floating-point number.",
    members: &[
        StdlibMember {
            name: "isFinite",
            kind: MemberKind::Property,
            signature: "isFinite: Boolean",
            doc: "True if this float is neither `NaN` nor infinity.",
        },
        StdlibMember {
            name: "isInfinite",
            kind: MemberKind::Property,
            signature: "isInfinite: Boolean",
            doc: "True if this float is positive or negative infinity.",
        },
        StdlibMember {
            name: "isNaN",
            kind: MemberKind::Property,
            signature: "isNaN: Boolean",
            doc: "True if this float is `NaN`.",
        },
        StdlibMember {
            name: "round",
            kind: MemberKind::Method,
            signature: "round(): Int",
            doc: "Round to the nearest integer.",
        },
        StdlibMember {
            name: "ceil",
            kind: MemberKind::Method,
            signature: "ceil(): Int",
            doc: "Round towards positive infinity.",
        },
        StdlibMember {
            name: "floor",
            kind: MemberKind::Method,
            signature: "floor(): Int",
            doc: "Round towards negative infinity.",
        },
    ],
};

// ----------------------------------------------------------------------
// Strings

static STRING: StdlibType = StdlibType {
    name: "String",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Sequence of Unicode characters.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of characters in this string.",
        },
        StdlibMember {
            name: "lastIndex",
            kind: MemberKind::Property,
            signature: "lastIndex: Int",
            doc: "Index of the last character (`length - 1`), or `-1` if empty.",
        },
        StdlibMember {
            name: "isEmpty",
            kind: MemberKind::Property,
            signature: "isEmpty: Boolean",
            doc: "True if this string has zero characters.",
        },
        StdlibMember {
            name: "isBlank",
            kind: MemberKind::Property,
            signature: "isBlank: Boolean",
            doc: "True if this string contains only whitespace characters.",
        },
        StdlibMember {
            name: "chars",
            kind: MemberKind::Property,
            signature: "chars: List<Char>",
            doc: "Characters of this string as a `List<Char>`.",
        },
        StdlibMember {
            name: "codePoints",
            kind: MemberKind::Property,
            signature: "codePoints: List<Int>",
            doc: "Unicode code points of this string.",
        },
        StdlibMember {
            name: "contains",
            kind: MemberKind::Method,
            signature: "contains(other: String): Boolean",
            doc: "True if `other` appears as a substring.",
        },
        StdlibMember {
            name: "startsWith",
            kind: MemberKind::Method,
            signature: "startsWith(prefix: String): Boolean",
            doc: "True if this string starts with `prefix`.",
        },
        StdlibMember {
            name: "endsWith",
            kind: MemberKind::Method,
            signature: "endsWith(suffix: String): Boolean",
            doc: "True if this string ends with `suffix`.",
        },
        StdlibMember {
            name: "indexOf",
            kind: MemberKind::Method,
            signature: "indexOf(needle: String): Int",
            doc: "Index of the first occurrence of `needle`, or `-1` if not found.",
        },
        StdlibMember {
            name: "lastIndexOf",
            kind: MemberKind::Method,
            signature: "lastIndexOf(needle: String): Int",
            doc: "Index of the last occurrence of `needle`, or `-1` if not found.",
        },
        StdlibMember {
            name: "indexOfOrNull",
            kind: MemberKind::Method,
            signature: "indexOfOrNull(needle: String): Int?",
            doc: "Like [`indexOf`] but returns `null` instead of `-1`.",
        },
        StdlibMember {
            name: "substring",
            kind: MemberKind::Method,
            signature: "substring(start: Int, end: Int): String",
            doc: "Substring from `start` (inclusive) to `end` (exclusive).",
        },
        StdlibMember {
            name: "take",
            kind: MemberKind::Method,
            signature: "take(n: Int): String",
            doc: "First `n` characters of this string.",
        },
        StdlibMember {
            name: "drop",
            kind: MemberKind::Method,
            signature: "drop(n: Int): String",
            doc: "This string with its first `n` characters removed.",
        },
        StdlibMember {
            name: "takeLast",
            kind: MemberKind::Method,
            signature: "takeLast(n: Int): String",
            doc: "Last `n` characters of this string.",
        },
        StdlibMember {
            name: "dropLast",
            kind: MemberKind::Method,
            signature: "dropLast(n: Int): String",
            doc: "This string with its last `n` characters removed.",
        },
        StdlibMember {
            name: "repeat",
            kind: MemberKind::Method,
            signature: "repeat(n: Int): String",
            doc: "Concatenate this string with itself `n` times.",
        },
        StdlibMember {
            name: "reverse",
            kind: MemberKind::Method,
            signature: "reverse(): String",
            doc: "Reverse the order of characters.",
        },
        StdlibMember {
            name: "trim",
            kind: MemberKind::Method,
            signature: "trim(): String",
            doc: "Remove leading and trailing whitespace.",
        },
        StdlibMember {
            name: "trimStart",
            kind: MemberKind::Method,
            signature: "trimStart(): String",
            doc: "Remove leading whitespace.",
        },
        StdlibMember {
            name: "trimEnd",
            kind: MemberKind::Method,
            signature: "trimEnd(): String",
            doc: "Remove trailing whitespace.",
        },
        StdlibMember {
            name: "padStart",
            kind: MemberKind::Method,
            signature: "padStart(width: Int, char: Char): String",
            doc: "Pad the start of this string with `char` until at least `width` characters long.",
        },
        StdlibMember {
            name: "padEnd",
            kind: MemberKind::Method,
            signature: "padEnd(width: Int, char: Char): String",
            doc: "Pad the end of this string with `char` until at least `width` characters long.",
        },
        StdlibMember {
            name: "capitalize",
            kind: MemberKind::Method,
            signature: "capitalize(): String",
            doc: "Upper-case the first character of this string.",
        },
        StdlibMember {
            name: "decapitalize",
            kind: MemberKind::Method,
            signature: "decapitalize(): String",
            doc: "Lower-case the first character of this string.",
        },
        StdlibMember {
            name: "toUpperCase",
            kind: MemberKind::Method,
            signature: "toUpperCase(): String",
            doc: "Upper-case every character.",
        },
        StdlibMember {
            name: "toLowerCase",
            kind: MemberKind::Method,
            signature: "toLowerCase(): String",
            doc: "Lower-case every character.",
        },
        StdlibMember {
            name: "replaceFirst",
            kind: MemberKind::Method,
            signature: "replaceFirst(target: String, replacement: String): String",
            doc: "Replace the first occurrence of `target` with `replacement`.",
        },
        StdlibMember {
            name: "replaceAll",
            kind: MemberKind::Method,
            signature: "replaceAll(target: String, replacement: String): String",
            doc: "Replace every occurrence of `target` with `replacement`.",
        },
        StdlibMember {
            name: "split",
            kind: MemberKind::Method,
            signature: "split(separator: String): List<String>",
            doc: "Split this string on every occurrence of `separator`.",
        },
        StdlibMember {
            name: "matches",
            kind: MemberKind::Method,
            signature: "matches(regex: Regex): Boolean",
            doc: "True if this string matches `regex` end-to-end.",
        },
        StdlibMember {
            name: "toInt",
            kind: MemberKind::Method,
            signature: "toInt(): Int",
            doc: "Parse this string as an integer; throws if invalid.",
        },
        StdlibMember {
            name: "toIntOrNull",
            kind: MemberKind::Method,
            signature: "toIntOrNull(): Int?",
            doc: "Parse this string as an integer, or `null` if invalid.",
        },
        StdlibMember {
            name: "toFloat",
            kind: MemberKind::Method,
            signature: "toFloat(): Float",
            doc: "Parse this string as a float; throws if invalid.",
        },
        StdlibMember {
            name: "toBoolean",
            kind: MemberKind::Method,
            signature: "toBoolean(): Boolean",
            doc: "Parse this string as a boolean; throws if invalid.",
        },
        StdlibMember {
            name: "md5",
            kind: MemberKind::Property,
            signature: "md5: String",
            doc: "Hex-encoded MD5 hash of this string's bytes.",
        },
        StdlibMember {
            name: "sha1",
            kind: MemberKind::Property,
            signature: "sha1: String",
            doc: "Hex-encoded SHA-1 hash of this string's bytes.",
        },
        StdlibMember {
            name: "sha256",
            kind: MemberKind::Property,
            signature: "sha256: String",
            doc: "Hex-encoded SHA-256 hash of this string's bytes.",
        },
        StdlibMember {
            name: "base64",
            kind: MemberKind::Property,
            signature: "base64: String",
            doc: "Base64-encoded form of this string's bytes.",
        },
        StdlibMember {
            name: "base64Decoded",
            kind: MemberKind::Property,
            signature: "base64Decoded: String",
            doc: "Decode this string from Base64 into UTF-8 bytes.",
        },
    ],
};

static CHAR: StdlibType = StdlibType {
    name: "Char",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "A single Unicode character.",
    members: &[
        StdlibMember {
            name: "codePoint",
            kind: MemberKind::Property,
            signature: "codePoint: Int",
            doc: "Unicode code point of this character.",
        },
        StdlibMember {
            name: "isLetter",
            kind: MemberKind::Property,
            signature: "isLetter: Boolean",
            doc: "True if this character is a Unicode letter.",
        },
        StdlibMember {
            name: "isDigit",
            kind: MemberKind::Property,
            signature: "isDigit: Boolean",
            doc: "True if this character is a Unicode digit.",
        },
    ],
};

static BYTES: StdlibType = StdlibType {
    name: "Bytes",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Immutable sequence of bytes.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of bytes.",
        },
        StdlibMember {
            name: "base64",
            kind: MemberKind::Property,
            signature: "base64: String",
            doc: "Base64-encoded form of these bytes.",
        },
        StdlibMember {
            name: "md5",
            kind: MemberKind::Property,
            signature: "md5: String",
            doc: "Hex-encoded MD5 hash.",
        },
        StdlibMember {
            name: "sha256",
            kind: MemberKind::Property,
            signature: "sha256: String",
            doc: "Hex-encoded SHA-256 hash.",
        },
    ],
};

// ----------------------------------------------------------------------
// Duration / DataSize

static DURATION: StdlibType = StdlibType {
    name: "Duration",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "A length of time. Construct with the [`Int`] unit suffixes \
          `1.s`, `200.ms`, `5.min`, etc.",
    members: &[
        StdlibMember {
            name: "value",
            kind: MemberKind::Property,
            signature: "value: Number",
            doc: "Numeric magnitude of this duration in its declared unit.",
        },
        StdlibMember {
            name: "unit",
            kind: MemberKind::Property,
            signature: "unit: String",
            doc: "Unit of measurement: `\"ns\"`, `\"us\"`, `\"ms\"`, `\"s\"`, `\"min\"`, `\"h\"`, or `\"d\"`.",
        },
        StdlibMember {
            name: "isoString",
            kind: MemberKind::Property,
            signature: "isoString: String",
            doc: "ISO 8601 representation, e.g. `PT5S`.",
        },
        StdlibMember {
            name: "toUnit",
            kind: MemberKind::Method,
            signature: "toUnit(unit: String): Duration",
            doc: "Convert to a different unit, preserving the duration.",
        },
    ],
};

static DATA_SIZE: StdlibType = StdlibType {
    name: "DataSize",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "A quantity of bytes. Construct with the [`Int`] unit suffixes \
          `512.b`, `2.mb`, `1.gib`, etc.",
    members: &[
        StdlibMember {
            name: "value",
            kind: MemberKind::Property,
            signature: "value: Number",
            doc: "Numeric magnitude of this size in its declared unit.",
        },
        StdlibMember {
            name: "unit",
            kind: MemberKind::Property,
            signature: "unit: String",
            doc: "Unit suffix: `\"b\"`, `\"kb\"`, `\"mb\"`, `\"gb\"`, `\"tb\"`, `\"pb\"`, \
                  `\"kib\"`, `\"mib\"`, `\"gib\"`, `\"tib\"`, `\"pib\"`.",
        },
        StdlibMember {
            name: "toUnit",
            kind: MemberKind::Method,
            signature: "toUnit(unit: String): DataSize",
            doc: "Convert to a different unit, preserving the size.",
        },
    ],
};

// ----------------------------------------------------------------------
// Pair

static PAIR: StdlibType = StdlibType {
    name: "Pair",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &["A", "B"],
    extends: Some("Any"),
    doc: "An ordered pair of two values.",
    members: &[
        StdlibMember {
            name: "first",
            kind: MemberKind::Property,
            signature: "first: A",
            doc: "First component.",
        },
        StdlibMember {
            name: "second",
            kind: MemberKind::Property,
            signature: "second: B",
            doc: "Second component.",
        },
    ],
};

// ----------------------------------------------------------------------
// Collections

static COLLECTION: StdlibType = StdlibType {
    name: "Collection",
    module: "pkl.base",
    kind: StdlibKind::AbstractClass,
    generics: &["T"],
    extends: Some("Any"),
    doc: "Abstract supertype of `List<T>` and `Set<T>`.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of elements.",
        },
        StdlibMember {
            name: "isEmpty",
            kind: MemberKind::Property,
            signature: "isEmpty: Boolean",
            doc: "True if there are no elements.",
        },
        StdlibMember {
            name: "first",
            kind: MemberKind::Property,
            signature: "first: T",
            doc: "First element; throws if empty.",
        },
        StdlibMember {
            name: "firstOrNull",
            kind: MemberKind::Property,
            signature: "firstOrNull: T?",
            doc: "First element, or `null` if empty.",
        },
        StdlibMember {
            name: "last",
            kind: MemberKind::Property,
            signature: "last: T",
            doc: "Last element; throws if empty.",
        },
        StdlibMember {
            name: "lastOrNull",
            kind: MemberKind::Property,
            signature: "lastOrNull: T?",
            doc: "Last element, or `null` if empty.",
        },
        StdlibMember {
            name: "single",
            kind: MemberKind::Property,
            signature: "single: T",
            doc: "Sole element; throws if not exactly one element.",
        },
        StdlibMember {
            name: "contains",
            kind: MemberKind::Method,
            signature: "contains(element: T): Boolean",
            doc: "True if `element` is in this collection.",
        },
        StdlibMember {
            name: "every",
            kind: MemberKind::Method,
            signature: "every(predicate: (T) -> Boolean): Boolean",
            doc: "True if `predicate` holds for every element.",
        },
        StdlibMember {
            name: "any",
            kind: MemberKind::Method,
            signature: "any(predicate: (T) -> Boolean): Boolean",
            doc: "True if `predicate` holds for at least one element.",
        },
        StdlibMember {
            name: "count",
            kind: MemberKind::Method,
            signature: "count(predicate: (T) -> Boolean): Int",
            doc: "Number of elements satisfying `predicate`.",
        },
        StdlibMember {
            name: "fold",
            kind: MemberKind::Method,
            signature: "fold<R>(initial: R, op: (R, T) -> R): R",
            doc: "Left fold over the elements.",
        },
        StdlibMember {
            name: "reduce",
            kind: MemberKind::Method,
            signature: "reduce(op: (T, T) -> T): T",
            doc: "Reduce using `op`; throws if empty.",
        },
        StdlibMember {
            name: "map",
            kind: MemberKind::Method,
            signature: "map<R>(transform: (T) -> R): List<R>",
            doc: "Apply `transform` to every element.",
        },
        StdlibMember {
            name: "filter",
            kind: MemberKind::Method,
            signature: "filter(predicate: (T) -> Boolean): List<T>",
            doc: "Keep only the elements satisfying `predicate`.",
        },
        StdlibMember {
            name: "filterNonNull",
            kind: MemberKind::Method,
            signature: "filterNonNull(): List<T>",
            doc: "Remove any `null` elements.",
        },
        StdlibMember {
            name: "flatMap",
            kind: MemberKind::Method,
            signature: "flatMap<R>(transform: (T) -> Collection<R>): List<R>",
            doc: "Apply `transform` and flatten one level.",
        },
        StdlibMember {
            name: "groupBy",
            kind: MemberKind::Method,
            signature: "groupBy<K>(key: (T) -> K): Map<K, List<T>>",
            doc: "Group elements by the result of `key`.",
        },
        StdlibMember {
            name: "take",
            kind: MemberKind::Method,
            signature: "take(n: Int): List<T>",
            doc: "First `n` elements.",
        },
        StdlibMember {
            name: "drop",
            kind: MemberKind::Method,
            signature: "drop(n: Int): List<T>",
            doc: "Skip the first `n` elements.",
        },
        StdlibMember {
            name: "sortBy",
            kind: MemberKind::Method,
            signature: "sortBy<K>(key: (T) -> K): List<T>",
            doc: "Sort ascending by `key`.",
        },
        StdlibMember {
            name: "distinctBy",
            kind: MemberKind::Method,
            signature: "distinctBy<K>(key: (T) -> K): List<T>",
            doc: "Remove duplicates by `key`.",
        },
        StdlibMember {
            name: "toList",
            kind: MemberKind::Method,
            signature: "toList(): List<T>",
            doc: "Convert to a `List<T>`.",
        },
        StdlibMember {
            name: "toSet",
            kind: MemberKind::Method,
            signature: "toSet(): Set<T>",
            doc: "Convert to a `Set<T>`.",
        },
        StdlibMember {
            name: "join",
            kind: MemberKind::Method,
            signature: "join(separator: String): String",
            doc: "Concatenate elements as strings separated by `separator`.",
        },
    ],
};

static LIST: StdlibType = StdlibType {
    name: "List",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &["T"],
    extends: Some("Collection<T>"),
    doc: "An immutable, ordered, indexable collection.",
    members: &[
        StdlibMember {
            name: "subList",
            kind: MemberKind::Method,
            signature: "subList(start: Int, end: Int): List<T>",
            doc: "Sub-list from `start` (inclusive) to `end` (exclusive).",
        },
        StdlibMember {
            name: "indexOf",
            kind: MemberKind::Method,
            signature: "indexOf(element: T): Int",
            doc: "Index of the first occurrence of `element`, or `-1`.",
        },
        StdlibMember {
            name: "add",
            kind: MemberKind::Method,
            signature: "add(element: T): List<T>",
            doc: "Return a new list with `element` appended.",
        },
        StdlibMember {
            name: "addAll",
            kind: MemberKind::Method,
            signature: "addAll(elements: Collection<T>): List<T>",
            doc: "Return a new list with `elements` appended.",
        },
        StdlibMember {
            name: "reverse",
            kind: MemberKind::Method,
            signature: "reverse(): List<T>",
            doc: "Return a new list with elements in reverse order.",
        },
    ],
};

static SET: StdlibType = StdlibType {
    name: "Set",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &["T"],
    extends: Some("Collection<T>"),
    doc: "An immutable, unordered collection of distinct elements.",
    members: &[
        StdlibMember {
            name: "add",
            kind: MemberKind::Method,
            signature: "add(element: T): Set<T>",
            doc: "Return a new set with `element` added.",
        },
        StdlibMember {
            name: "remove",
            kind: MemberKind::Method,
            signature: "remove(element: T): Set<T>",
            doc: "Return a new set without `element`.",
        },
        StdlibMember {
            name: "union",
            kind: MemberKind::Method,
            signature: "union(other: Collection<T>): Set<T>",
            doc: "Union with `other`.",
        },
        StdlibMember {
            name: "intersect",
            kind: MemberKind::Method,
            signature: "intersect(other: Collection<T>): Set<T>",
            doc: "Intersection with `other`.",
        },
    ],
};

static MAP: StdlibType = StdlibType {
    name: "Map",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &["K", "V"],
    extends: Some("Any"),
    doc: "An immutable mapping from keys to values.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of entries.",
        },
        StdlibMember {
            name: "isEmpty",
            kind: MemberKind::Property,
            signature: "isEmpty: Boolean",
            doc: "True if there are no entries.",
        },
        StdlibMember {
            name: "keys",
            kind: MemberKind::Property,
            signature: "keys: Set<K>",
            doc: "Set of all keys.",
        },
        StdlibMember {
            name: "values",
            kind: MemberKind::Property,
            signature: "values: List<V>",
            doc: "List of all values, in insertion order.",
        },
        StdlibMember {
            name: "entries",
            kind: MemberKind::Property,
            signature: "entries: List<Pair<K, V>>",
            doc: "List of all `(key, value)` pairs.",
        },
        StdlibMember {
            name: "containsKey",
            kind: MemberKind::Method,
            signature: "containsKey(key: K): Boolean",
            doc: "True if `key` is present in this map.",
        },
        StdlibMember {
            name: "getOrNull",
            kind: MemberKind::Method,
            signature: "getOrNull(key: K): V?",
            doc: "Value for `key`, or `null` if absent.",
        },
        StdlibMember {
            name: "getOrDefault",
            kind: MemberKind::Method,
            signature: "getOrDefault(key: K, default: V): V",
            doc: "Value for `key`, or `default` if absent.",
        },
        StdlibMember {
            name: "put",
            kind: MemberKind::Method,
            signature: "put(key: K, value: V): Map<K, V>",
            doc: "Return a new map with `key` mapped to `value`.",
        },
        StdlibMember {
            name: "remove",
            kind: MemberKind::Method,
            signature: "remove(key: K): Map<K, V>",
            doc: "Return a new map without `key`.",
        },
        StdlibMember {
            name: "mapKeys",
            kind: MemberKind::Method,
            signature: "mapKeys<K2>(transform: (K) -> K2): Map<K2, V>",
            doc: "Transform every key.",
        },
        StdlibMember {
            name: "mapValues",
            kind: MemberKind::Method,
            signature: "mapValues<V2>(transform: (V) -> V2): Map<K, V2>",
            doc: "Transform every value.",
        },
        StdlibMember {
            name: "filter",
            kind: MemberKind::Method,
            signature: "filter(predicate: (K, V) -> Boolean): Map<K, V>",
            doc: "Keep only the entries satisfying `predicate`.",
        },
    ],
};

// ----------------------------------------------------------------------
// Pkl-flavoured collections that show up in user files

static LISTING: StdlibType = StdlibType {
    name: "Listing",
    module: "pkl.base",
    kind: StdlibKind::OpenClass,
    generics: &["T"],
    extends: Some("Any"),
    doc: "An open-ended list literal, the value of `new Listing<T> { ... }`.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of elements declared.",
        },
        StdlibMember {
            name: "isEmpty",
            kind: MemberKind::Property,
            signature: "isEmpty: Boolean",
            doc: "True if there are no elements.",
        },
        StdlibMember {
            name: "toList",
            kind: MemberKind::Method,
            signature: "toList(): List<T>",
            doc: "Convert to a regular `List<T>`.",
        },
    ],
};

static MAPPING: StdlibType = StdlibType {
    name: "Mapping",
    module: "pkl.base",
    kind: StdlibKind::OpenClass,
    generics: &["K", "V"],
    extends: Some("Any"),
    doc: "An open-ended map literal, the value of `new Mapping<K, V> { ... }`.",
    members: &[
        StdlibMember {
            name: "length",
            kind: MemberKind::Property,
            signature: "length: Int",
            doc: "Number of entries declared.",
        },
        StdlibMember {
            name: "toMap",
            kind: MemberKind::Method,
            signature: "toMap(): Map<K, V>",
            doc: "Convert to a regular `Map<K, V>`.",
        },
    ],
};

static DYNAMIC: StdlibType = StdlibType {
    name: "Dynamic",
    module: "pkl.base",
    kind: StdlibKind::OpenClass,
    generics: &[],
    extends: Some("Any"),
    doc: "An object whose schema is not known statically.",
    members: &[
        StdlibMember {
            name: "toMap",
            kind: MemberKind::Method,
            signature: "toMap(): Map<String, Any>",
            doc: "Convert into a `Map<String, Any>`.",
        },
        StdlibMember {
            name: "toDynamic",
            kind: MemberKind::Method,
            signature: "toDynamic(): Dynamic",
            doc: "Return this value as a `Dynamic`.",
        },
    ],
};

static REGEX: StdlibType = StdlibType {
    name: "Regex",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "A compiled regular expression.",
    members: &[StdlibMember {
        name: "pattern",
        kind: MemberKind::Property,
        signature: "pattern: String",
        doc: "Original pattern source.",
    }],
};

static MODULE_TYPE: StdlibType = StdlibType {
    name: "Module",
    module: "pkl.base",
    kind: StdlibKind::OpenClass,
    generics: &[],
    extends: Some("Any"),
    doc: "The implicit type of a Pkl module's top-level bindings.",
    members: &[],
};

static CLASS_TYPE: StdlibType = StdlibType {
    name: "Class",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Runtime representation of a class.",
    members: &[StdlibMember {
        name: "name",
        kind: MemberKind::Property,
        signature: "name: String",
        doc: "Fully qualified name of this class.",
    }],
};

static TYPE_ALIAS_TYPE: StdlibType = StdlibType {
    name: "TypeAlias",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "Runtime representation of a type alias.",
    members: &[],
};

static RESOURCE: StdlibType = StdlibType {
    name: "Resource",
    module: "pkl.base",
    kind: StdlibKind::Class,
    generics: &[],
    extends: Some("Any"),
    doc: "A resource read with `read()` or `read?()`.",
    members: &[
        StdlibMember {
            name: "uri",
            kind: MemberKind::Property,
            signature: "uri: String",
            doc: "URI the resource was loaded from.",
        },
        StdlibMember {
            name: "text",
            kind: MemberKind::Property,
            signature: "text: String",
            doc: "Resource contents decoded as UTF-8.",
        },
        StdlibMember {
            name: "base64",
            kind: MemberKind::Property,
            signature: "base64: String",
            doc: "Resource contents as a Base64-encoded string.",
        },
    ],
};

/// All types modeled from `pkl.base`. The analyzer registers each of these
/// as a synthetic symbol in the module scope.
pub static ALL: &[&StdlibType] = &[
    &ANY,
    &NULL_TYPE,
    &BOOLEAN,
    &NUMBER,
    &INT,
    &FLOAT,
    &STRING,
    &CHAR,
    &BYTES,
    &DURATION,
    &DATA_SIZE,
    &PAIR,
    &COLLECTION,
    &LIST,
    &SET,
    &MAP,
    &LISTING,
    &MAPPING,
    &DYNAMIC,
    &REGEX,
    &MODULE_TYPE,
    &CLASS_TYPE,
    &TYPE_ALIAS_TYPE,
    &RESOURCE,
];
