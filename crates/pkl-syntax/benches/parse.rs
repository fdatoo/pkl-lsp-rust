//! Lex + parse benchmarks for representative Pkl inputs.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

/// A small but realistic Pkl module.
const SMALL: &str = r#"
module acme.config

import "pkl:json" as json
import "./util.pkl" as util

class Server {
  host: String = "localhost"
  port: Int = 8080
  tags: List<String> = List("primary", "fallback")
}

servers: List<Server> = List(
  new Server { host = "a" },
  new Server { host = "b"; port = 9090 },
)

function describe(s: Server): String = "\(s.host):\(s.port)"
"#;

/// A larger synthetic module: repeats the `SMALL` body 32 times.
fn synthesize_large() -> String {
    let mut out = String::with_capacity(SMALL.len() * 32);
    for _ in 0..32 {
        out.push_str(SMALL);
        out.push('\n');
    }
    out
}

fn bench_lex(c: &mut Criterion) {
    let large = synthesize_large();
    let mut group = c.benchmark_group("lex");
    group.throughput(Throughput::Bytes(SMALL.len() as u64));
    group.bench_function("small", |b| {
        b.iter(|| {
            let toks = pkl_syntax::tokenize(black_box(SMALL));
            black_box(toks.len());
        })
    });
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("large_32x", |b| {
        b.iter(|| {
            let toks = pkl_syntax::tokenize(black_box(&large));
            black_box(toks.len());
        })
    });
    group.finish();
}

fn bench_parse(c: &mut Criterion) {
    let large = synthesize_large();
    let mut group = c.benchmark_group("parse");
    group.throughput(Throughput::Bytes(SMALL.len() as u64));
    group.bench_function("small", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(SMALL));
            use pkl_syntax::cst::{AstNode, Module};
            let module = Module::cast(r.syntax()).unwrap();
            black_box(module.items().count());
        })
    });
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("large_32x", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(&large));
            use pkl_syntax::cst::{AstNode, Module};
            let module = Module::cast(r.syntax()).unwrap();
            black_box(module.items().count());
        })
    });
    group.finish();
}

fn bench_stdlib_base(c: &mut Criterion) {
    // Parse the real `pkl.base` source to track regressions on a hot
    // analyzer path.
    let source = include_str!("../../pkl-stdlib/vendor/base.pkl");
    let mut group = c.benchmark_group("parse_stdlib");
    group.throughput(Throughput::Bytes(source.len() as u64));
    group.bench_function("base.pkl", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(source));
            use pkl_syntax::cst::{AstNode, Module};
            let module = Module::cast(r.syntax()).unwrap();
            black_box(module.items().count());
        })
    });
    group.finish();
}

criterion_group!(benches, bench_lex, bench_parse, bench_stdlib_base);
criterion_main!(benches);
