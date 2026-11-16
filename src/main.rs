use lazierdocker::app::run::{run, Options};

const USAGE: &str = "lazierdocker — a mouse-first terminal UI for Docker

Usage: lazierdocker [OPTIONS]

Options:
  -H, --host <HOST>      Docker host (default: $DOCKER_HOST, then the current docker
                         context, then unix:///var/run/docker.sock)
  -p, --project <NAME>   Compose project name for the current directory
  -f, --file <FILE>      Compose file (repeatable)
  -h, --help             Show this help
  -V, --version          Show version";

fn parse_args() -> Result<Options, String> {
    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} requires a value"));
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("lazierdocker {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-H" | "--host" => opts.host = Some(value("--host")?),
            "-p" | "--project" => opts.project = Some(value("--project")?),
            "-f" | "--file" => opts.compose_files.push(value("--file")?),
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }
    Ok(opts)
}

#[tokio::main]
async fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(opts).await {
        eprintln!("lazierdocker: {e:#}");
        std::process::exit(1);
    }
}
