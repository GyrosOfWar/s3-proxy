extern crate actix;
extern crate actix_web;
extern crate rusoto_core as aws;
extern crate rusoto_s3 as s3;
extern crate serde;
extern crate toml;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate log;
extern crate bytes;
extern crate failure;
extern crate futures;
extern crate log4rs;

use std::sync::Arc;

use actix_web::{http::{ContentEncoding, StatusCode},
                server,
                App,
                Body,
                HttpMessage,
                HttpRequest,
                HttpResponse};
use failure::Error;
use futures::{Future, Stream};

type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Config {
    pub port: u16,
    pub host: String,
    pub bucket: String,
    pub region: String,
}

fn read_config() -> Result<Config> {
    const CONFIG_FILE: &str = "s3-proxy.toml";

    use std::fs::File;
    use std::io::Read;

    let mut content = String::new();
    let mut file = File::open(CONFIG_FILE)?;
    file.read_to_string(&mut content)?;

    toml::from_str(&content).map_err(From::from)
}

fn configure_logger() {
    const LOGGER_CONFIG: &str = "log4rs.yml";

    if let Err(e) = log4rs::init_file(LOGGER_CONFIG, Default::default()) {
        eprintln!("Failed to set logger: {}", e);
    }
}

struct State {
    s3_client: Arc<s3::S3Client>,
    config: Config,
}

fn handler(req: HttpRequest<State>) -> Box<Future<Item = HttpResponse, Error = Error>> {
    use bytes::Bytes;
    use s3::S3;

    // TODO handle missing keys (404)
    // TODO reject empty keys
    let s3_key = &req.path()[1..];
    debug!("Handling request to {}", s3_key);
    let client = Arc::clone(&req.state().s3_client);
    let config = &req.state().config;
    let range = req.headers()
        .get("Range")
        .and_then(|r| r.to_str().ok())
        .map(From::from);

    debug!("Request headers: {:?}", req.headers());
    let resp = client
        .get_object(&s3::GetObjectRequest {
            bucket: config.bucket.clone(),
            key: s3_key.into(),
            range,
            ..Default::default()
        })
        .from_err()
        .map(|res| {
            debug!("S3 response: {:?}", res);

            let body = res.body
                .expect("No body for response")
                .map(Bytes::from)
                .map_err(Error::from);
            let mut builder = HttpResponse::Ok();

            if let Some(content_length) = res.content_length {
                debug!("Content-Length: {}", content_length);
                builder.content_length(content_length as u64);
            }
            if let Some(content_type) = res.content_type {
                if content_type.starts_with("audio") || content_type.starts_with("video")
                    || content_type.starts_with("image")
                {
                    builder.content_encoding(ContentEncoding::Identity);
                }
                debug!("Content-Type: {}", content_type);
                builder.content_type(content_type.as_str());
            }
            if let Some(e_tag) = res.e_tag {
                builder.header("ETag", e_tag);
            }
            if let Some(content_range) = res.content_range {
                debug!("Content-Range: {}", content_range);
                builder.header("Content-Range", content_range);
                builder.status(StatusCode::PARTIAL_CONTENT);
            }
            if let Some(accept_ranges) = res.accept_ranges {
                debug!("Accept-Ranges: {}", accept_ranges);
                builder.header("Accept-Ranges", accept_ranges);
            }
            if let Some(last_modified) = res.last_modified {
                builder.header("Last-Modified", last_modified);
            }

            debug!("--- Sending request --- ");
            builder.body(Body::Streaming(Box::new(body.map_err(From::from))))
        });

    Box::new(resp)
}

fn run() -> Result<()> {
    configure_logger();
    let config = read_config()?;
    let region = config.region.parse()?;
    let s3_client = Arc::new(s3::S3Client::simple(region));
    let addr = format!("{}:{}", config.host, config.port);

    server::new(move || {
        App::with_state(State {
            s3_client: Arc::clone(&s3_client),
            config: config.clone(),
        }).resource("/{path:.+}", |r| r.f(handler))
    }).bind(addr)?
        .run();

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}
