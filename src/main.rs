extern crate actix_web;
extern crate bytes;
extern crate failure;
extern crate futures;
#[macro_use]
extern crate log;
extern crate log4rs;
extern crate rusoto_core as aws;
extern crate rusoto_s3 as s3;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate mime_guess;
extern crate num_cpus;
extern crate toml;

use std::path::Path;
use std::sync::Arc;

use actix_web::{
    http::Method, http::{ContentEncoding, StatusCode}, server, App, Body, HttpMessage, HttpRequest,
    HttpResponse, Path as UrlPath,
};
use failure::Error;
use futures::{
    future::{self, Either}, Future, Stream,
};

trait OptionExt<T> {
    fn filter_val<P: FnOnce(&T) -> bool>(self, predicate: P) -> Self;
}

impl<T> OptionExt<T> for Option<T> {
    fn filter_val<P: FnOnce(&T) -> bool>(self, predicate: P) -> Self {
        if let Some(x) = self {
            if predicate(&x) {
                return Some(x);
            }
        }
        None
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Config {
    /// Host to bind to
    pub host: String,

    /// Port to bind to
    pub port: u16,

    /// Which bucket, if any, to use
    pub bucket: String,

    /// Which AWS region to use.
    pub region: String,

    /// Number of actix workers
    pub workers: Option<usize>,
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

fn handle_response(res: s3::GetObjectOutput, key: String) -> HttpResponse {
    use bytes::Bytes;
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
        // Don't gzip media files
        if content_type.starts_with("audio")
            || content_type.starts_with("video")
            || content_type.starts_with("image")
        {
            debug!("not GZIPping media file");
            builder.content_encoding(ContentEncoding::Identity);
        }
        if content_type == "binary/octet-stream" || content_type == "application/octet-stream" {
            if let Some(extension) = Path::new(&key).extension().and_then(|s| s.to_str()) {
                debug!("File has extension {}", extension);
                let mime = mime_guess::get_mime_type(extension);
                let mime = mime.as_ref();
                debug!("Determined file type {} from extension", mime);
                builder.content_type(mime);
            }
        } else {
            debug!("Content-Type: {}", content_type);
            builder.content_type(content_type.as_str());
        }
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

    builder.header("Cache-Control", "public, max-age=31536000");

    debug!("--- Sending request --- ");
    builder.body(Body::Streaming(Box::new(body.map_err(From::from))))
}

fn handler(
    (req, path): (HttpRequest<State>, UrlPath<String>),
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    use s3::S3;

    let client = Arc::clone(&req.state().s3_client);
    let config = &req.state().config;
    let range = req.headers()
        .get("Range")
        .and_then(|r| r.to_str().ok())
        .map(From::from);

    let key = path.into_inner();
    let bucket = config.bucket.clone();

    if key.is_empty() {
        return Box::new(future::ok(HttpResponse::NotFound().body("404 - Not found")));
    }

    debug!("Request headers: {:?}", req.headers());
    let resp = client
        .get_object(s3::GetObjectRequest {
            bucket,
            key: key.clone(),
            range,
            ..Default::default()
        })
        .then(|result| {
            if let Err(s3::GetObjectError::NoSuchKey(_)) = result {
                Ok(Either::B(HttpResponse::NotFound().body("404 - Not found")))
            } else {
                result.map(Either::A)
            }
        })
        .from_err()
        .map(|res| match res {
            Either::A(res) => handle_response(res, key),
            Either::B(res) => res,
        });

    Box::new(resp)
}

fn run() -> Result<()> {
    use actix_web::middleware;
    use std::env;

    configure_logger();
    let mut config = read_config()?;

    if let Ok(bucket) = env::var("S3_BUCKET") {
        config.bucket = bucket;
    }

    if let Some(port) = env::var("S3_PROXY_PORT").ok().and_then(|p| p.parse().ok()) {
        config.port = port;
    }

    info!("Hosting content from bucket '{}' ", config.bucket);

    let region = config.region.parse()?;
    let s3_client = Arc::new(s3::S3Client::new(region));
    let workers = config.workers;
    let addr = format!("{}:{}", config.host, config.port);

    server::new(move || {
        App::with_state(State {
            s3_client: Arc::clone(&s3_client),
            config: config.clone(),
        }).middleware(middleware::Logger::new(r#"%t "%r" %s %b %T"#))
            .route("/{path:.*}", Method::GET, handler)
            .route("/{path:.*}", Method::HEAD, handler)
    }).workers(workers.unwrap_or_else(|| num_cpus::get()))
        .bind(addr)?
        .run();

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}
