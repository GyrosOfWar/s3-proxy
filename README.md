# s3-proxy
Tiny Rust proxy meant to proxy requests to a configured S3 bucket. 

## Configuration
Create a configuration file called `s3-proxy.toml`. Example:
```toml
host = "localhost"
port = 3221
bucket = "<my-bucket-name>"
region = "eu-central-1"
```
AWS credentials are read via `rusoto_credentials` default mechanism. 
