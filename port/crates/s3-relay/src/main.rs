//! `s3-relay [--listen ADDR] [--allow-exe SHA256]... [--gog-only] [--grace SECS] [-v]`
//!
//! Defaults: listen 127.0.0.1:7777, any exe (sessions still only match identical exes).

use std::time::Duration;

struct StderrLog(log::LevelFilter);

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            eprintln!("{t} {:5} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn usage() -> ! {
    eprintln!("usage: s3-relay [--listen ADDR:PORT] [--allow-exe SHA256]... [--gog-only] [--grace SECS] [-v]");
    std::process::exit(2);
}

fn main() {
    let mut cfg = s3_relay::RelayConfig { listen: "127.0.0.1:7777".parse().unwrap(), ..Default::default() };
    let mut level = log::LevelFilter::Info;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => cfg.listen = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--allow-exe" => cfg.allowed_exes.push(args.next().and_then(|v| s3_net::parse_sha256(&v)).unwrap_or_else(|| usage())),
            "--gog-only" => cfg.allowed_exes.push(s3_net::GOG_EXE_SHA256),
            "--grace" => {
                cfg.resume_grace = Duration::from_secs(args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()))
            }
            "-v" => level = log::LevelFilter::Debug,
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let logger: &'static StderrLog = Box::leak(Box::new(StderrLog(level)));
    let _ = log::set_logger(logger);
    log::set_max_level(level);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    rt.block_on(async move {
        let handle = match s3_relay::start(cfg).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!("s3-relay: cannot listen: {e}");
                std::process::exit(1);
            }
        };
        eprintln!("s3-relay: {}", handle.url());
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("s3-relay: shutting down ({:?})", handle.stats().await);
    });
}
