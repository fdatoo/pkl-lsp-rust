//! End-to-end analyzer benchmarks: parse + resolve + infer.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

const SMALL: &str = r#"
module acme.config

class Server {
  host: String = "localhost"
  port: Int = 8080
}

servers: List<Server> = List(
  new Server { host = "a" },
  new Server { host = "b"; port = 9090 },
)

function describe(s: Server): String = s.host
"#;

fn synthesize(n: usize) -> String {
    let mut out = String::with_capacity(SMALL.len() * n);
    for _ in 0..n {
        out.push_str(SMALL);
        out.push('\n');
    }
    out
}

fn bench_analyze(c: &mut Criterion) {
    let large = synthesize(16);
    let stdlib = include_str!("../../pkl-stdlib/vendor/base.pkl");
    let mut group = c.benchmark_group("analyze");
    group.throughput(Throughput::Bytes(SMALL.len() as u64));
    group.bench_function("small", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(SMALL));
            let a = pkl_analyze::analyze(&r.module, r.diagnostics);
            black_box(a.resolution.symbols.len());
        })
    });
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("large_16x", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(&large));
            let a = pkl_analyze::analyze(&r.module, r.diagnostics);
            black_box(a.resolution.symbols.len());
        })
    });
    group.throughput(Throughput::Bytes(stdlib.len() as u64));
    group.bench_function("pkl.base", |b| {
        b.iter(|| {
            let r = pkl_syntax::parse(black_box(stdlib));
            let a = pkl_analyze::analyze(&r.module, r.diagnostics);
            black_box(a.resolution.symbols.len());
        })
    });
    group.finish();
}

criterion_group!(benches, bench_analyze);
criterion_main!(benches);
