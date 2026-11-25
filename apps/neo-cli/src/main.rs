//! NEO command line. Cold-start, import, and plugin demo entry point outside the desktop installer.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use neo_cli::{check_update, imagery_example, open_path, open_synth, presets_markdown, scene_bench};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let result = match cmd.as_str() {
        "open" => cmd_open(&mut args),
        "scene" => cmd_scene(&mut args),
        "presets" => {
            println!("{}", presets_markdown());
            Ok(())
        }
        "imagery" => {
            let z = args.next().and_then(|v| v.parse().ok()).unwrap_or(12);
            let x = args.next().and_then(|v| v.parse().ok()).unwrap_or(2100);
            let y = args.next().and_then(|v| v.parse().ok()).unwrap_or(1400);
            println!("{}", imagery_example(z, x, y));
            Ok(())
        }
        "update-check" => cmd_update(&mut args),
        "plugins" => cmd_plugins(),
        _ => {
            eprintln!(
                "neo-cli\n  open <file>|--synth <n>\n  scene --nodes <n>\n  presets\n  imagery [z x y]\n  update-check --current <ver> --manifest <file>\n  plugins"
            );
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("neo-cli: {err}");
            ExitCode::from(1)
        }
    }
}

fn cmd_open(args: &mut impl Iterator<Item = String>) -> anyhow::Result<()> {
    let mut synth = None;
    let mut path = None;
    while let Some(arg) = args.next() {
        if arg == "--synth" {
            synth = args.next().and_then(|v| v.parse().ok());
        } else {
            path = Some(PathBuf::from(arg));
        }
    }
    let report = if let Some(n) = synth {
        open_synth(n)?
    } else if let Some(path) = path {
        open_path(&path)?
    } else {
        anyhow::bail!("open needs a file or --synth <n>");
    };
    println!(
        "source={} nodes={} ways={} relations={}",
        report.source, report.nodes, report.ways, report.relations
    );
    Ok(())
}

fn cmd_scene(args: &mut impl Iterator<Item = String>) -> anyhow::Result<()> {
    let mut nodes = 50_000usize;
    while let Some(arg) = args.next() {
        if arg == "--nodes" {
            nodes = args.next().and_then(|v| v.parse().ok()).unwrap_or(nodes);
        }
    }
    let report = scene_bench(nodes)?;
    println!(
        "nodes={nodes} vertices={} build_micros={} stream_builds={}",
        report.vertices, report.build_micros, report.builds
    );
    Ok(())
}

fn cmd_update(args: &mut impl Iterator<Item = String>) -> anyhow::Result<()> {
    let mut current = env!("CARGO_PKG_VERSION").to_owned();
    let mut manifest = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--current" => current = args.next().unwrap_or(current),
            "--manifest" => manifest = args.next(),
            _ => {}
        }
    }
    let Some(path) = manifest else {
        anyhow::bail!("update-check needs --manifest <file>");
    };
    let text = std::fs::read_to_string(path)?;
    match check_update(&current, &text)? {
        Some(offer) => println!("update {} -> {} {}", offer.current, offer.latest, offer.url),
        None => println!("up to date {current}"),
    }
    Ok(())
}

fn cmd_plugins() -> anyhow::Result<()> {
    #[cfg(feature = "plugins")]
    {
        println!("{}", neo_plugin::run_builtin_demo()?);
        Ok(())
    }
    #[cfg(not(feature = "plugins"))]
    {
        anyhow::bail!("this neo-cli was built without --features plugins");
    }
}
