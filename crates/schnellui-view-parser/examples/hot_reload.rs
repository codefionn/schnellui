//! cargo run -p schnellui-view-parser --example hot_reload -- [--watch] BODY_FILE SITE_ID ADDRESS
//! BODY_FILE contains one view! body. SITE_ID is compiled file:line:column.
use std::io::{BufRead, BufReader, Write};
fn send(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let body = std::fs::read_to_string(&args[0])?;
    let tokens = body
        .parse::<proc_macro2::TokenStream>()
        .map_err(|e| e.to_string())?;
    let tree = schnellui_view_parser::parse_view(tokens)?;
    let blueprint = tree.blueprint(&args[1]);
    let mut stream = std::net::TcpStream::connect(&args[2])?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    serde_json::to_writer(&mut stream, &blueprint)?;
    stream.write_all(b"\n")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    print!("{response}");
    if !response.starts_with("ok ") {
        return Err("template replacement rejected".into());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let watch = args.first().is_some_and(|arg| arg == "--watch");
    if watch {
        args.remove(0);
    }
    if args.len() != 3 {
        return Err("usage: hot_reload [--watch] BODY_FILE FILE:LINE:COLUMN HOST:PORT".into());
    }
    if !watch {
        return send(&args);
    }
    let mut previous = None;
    loop {
        let modified = std::fs::metadata(&args[0]).and_then(|metadata| metadata.modified());
        match modified {
            Ok(modified) if previous != Some(modified) => {
                previous = Some(modified);
                if let Err(error) = send(&args) {
                    eprintln!("{error}");
                }
            }
            Err(error) => eprintln!("{}: {error}", args[0]),
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
