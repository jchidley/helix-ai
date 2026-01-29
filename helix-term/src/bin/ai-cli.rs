use std::env;
use std::io::{self, Read};

use anyhow::{anyhow, Result};

use helix_term::ai::{
    config::{self, AiConfig},
    driver::Driver,
    format, session,
};

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let command = args.first().cloned().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        "list" => {
            args.remove(0);
            cmd_list(args)
        }
        "render" => {
            args.remove(0);
            cmd_render()
        }
        "driver" => {
            args.remove(0);
            cmd_driver(args)
        }
        _ => Err(anyhow!("unknown command '{command}'")),
    }
}

fn cmd_list(args: Vec<String>) -> Result<()> {
    let config = AiConfig::load()?;
    let name = args.get(0).map(|s| s.as_str()).unwrap_or(&config.general.default_driver);
    let driver_cfg = config::require_driver(&config, name)?;
    let driver = Driver::from_config(name, driver_cfg);
    let sessions = session::list_sessions(&driver)?;

    for entry in sessions {
        if let Some(id) = entry.session_id {
            println!("{}\t{}", entry.display, id);
        } else {
            println!("{}", entry.display);
        }
    }

    Ok(())
}

fn cmd_render() -> Result<()> {
    let config = AiConfig::load()?;
    let driver_cfg = config::require_driver(&config, &config.general.default_driver)?;
    let driver = Driver::from_config(&config.general.default_driver, driver_cfg);
    let mut formatter = format::Formatter::new(driver.kind, format::FormatMode::SessionFile);

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    for line in input.lines() {
        if let Some(rendered) = formatter.render_line(line) {
            if rendered.ends_with('\n') {
                print!("{}", rendered);
            } else {
                println!("{}", rendered);
            }
        }
    }
    Ok(())
}

fn cmd_driver(args: Vec<String>) -> Result<()> {
    let config = AiConfig::load()?;
    let name = args.get(0).map(|s| s.as_str()).unwrap_or(&config.general.default_driver);
    let driver_cfg = config::require_driver(&config, name)?;
    let driver = Driver::from_config(name, driver_cfg);

    println!("name: {}", driver.name);
    println!("mode: {:?}", driver.mode);
    println!("cmd: {}", driver.cmd.join(" "));
    println!("session_dir: {}", driver.session_dir.display());
    println!("session_glob: {}", driver.session_glob);

    Ok(())
}

fn print_usage() {
    eprintln!("hx-ai <command> [args]\n\nCommands:\n  list [driver]    List sessions for driver\n  render           Render JSONL from stdin\n  driver [driver]  Show driver config\n");
}
