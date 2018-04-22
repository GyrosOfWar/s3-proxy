# s3-proxy
Tiny Rust proxy meant to proxy requests to a configured S3 bucket. 
Uses actix-web for HTTP and rusoto for AWS. 

## Configuration
Requires a configuration file called `s3-proxy.toml`. Example:
```toml
host = "localhost"
port = 3221
# Optional
bucket = "<my-bucket-name>"
region = "eu-central-1"
url_prefix = "s3-files"
```
AWS credentials are read via `rusoto_credentials` default mechanism. 
URLs are built like this: `/{url-prefix}/{bucket}/{s3-key}`, where `bucket` and 
`url-prefix` are optional. (either can be provided via configuration).