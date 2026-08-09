use std::env;
use std::io;
use std::path::PathBuf;

use dcc_cua_bazaar_profile_companion::CompanionRuntime;
use tiny_http::{Header, Response, Server, StatusCode};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let config_path = flag_value(&args, "--config")
        .map(PathBuf::from)
        .ok_or("companion requires --config PATH")?;
    let mut runtime = CompanionRuntime::from_config_path(&config_path)?;
    if args.iter().any(|arg| arg == "--once") {
        println!("{}", serde_json::to_string_pretty(&runtime.poll()?)?);
        return Ok(());
    }

    let listen = runtime.listen().to_owned();
    let server = Server::http(&listen).map_err(io::Error::other)?;
    eprintln!("dcc-cua Bazaar profile companion listening on http://{listen}/v1/context");
    for request in server.incoming_requests() {
        if request.method().as_str() != "GET" || request.url() != "/v1/context" {
            request.respond(Response::empty(StatusCode(404)))?;
            continue;
        }
        let context = match runtime.poll() {
            Ok(context) => context,
            Err(error) => {
                request.respond(
                    Response::from_string(
                        serde_json::json!({"error": error.to_string()}).to_string(),
                    )
                    .with_status_code(StatusCode(503))
                    .with_header(json_header()),
                )?;
                continue;
            }
        };
        let etag = format!("\"tick-{}\"", context.tick_id);
        let not_modified = request
            .headers()
            .iter()
            .any(|header| header.field.equiv("If-None-Match") && header.value.as_str() == etag);
        if not_modified {
            request.respond(Response::empty(StatusCode(304)).with_header(etag_header(&etag)))?;
            continue;
        }
        request.respond(
            Response::from_data(serde_json::to_vec(&context)?)
                .with_header(json_header())
                .with_header(etag_header(&etag))
                .with_header(Header::from_bytes("Cache-Control", "no-cache").expect("header")),
        )?;
    }
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn json_header() -> Header {
    Header::from_bytes("Content-Type", "application/json; charset=utf-8").expect("header")
}

fn etag_header(etag: &str) -> Header {
    Header::from_bytes("ETag", etag).expect("header")
}
