use std::path::PathBuf;

use dnf_generator::generate::Generate;

/// The generator bootstrap.
///
/// CLI:
///   dnf-generator <command> [--workdir <path>] [--debug]
///
/// `<command>` is one of `hosts`, `users`, `network`, `disko`, `doc`.
/// `--workdir` defaults to the current working directory. The generator
/// reads `<workdir>/etc/config.yaml` and writes
/// `<workdir>/var/generated/*.nix`.
fn main() {
    let mut workdir: Option<PathBuf> = None;
    let mut command: Option<String> = None;
    let mut debug = false;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workdir" | "-w" => {
                workdir = iter.next().map(PathBuf::from);
            }
            "--debug" | "debug" => debug = true,
            other if other.starts_with("--workdir=") => {
                workdir = Some(PathBuf::from(&other["--workdir=".len()..]));
            }
            _ if command.is_none() => command = Some(arg),
            _ => {
                eprintln!("ERR: unexpected argument: {arg}");
                std::process::exit(2);
            }
        }
    }

    let workdir = workdir.unwrap_or_else(|| {
        std::env::current_dir().expect("Cannot determine current working directory")
    });
    let command = command.unwrap_or_else(|| "?".to_string());

    let main_yaml = workdir.join("etc/config.yaml");
    let generated_yaml = workdir.join("var/generated/config.yaml");

    match Generate::new(&main_yaml, &generated_yaml) {
        Ok(generator) => match generator.run(&command) {
            Ok(output) => print!("{output}"),
            Err(e) => {
                eprintln!("ERR: {e}");
                if debug {
                    eprintln!("{e:?}");
                }
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("ERR: {e}");
            if debug {
                eprintln!("{e:?}");
            }
            std::process::exit(1);
        }
    }
}
