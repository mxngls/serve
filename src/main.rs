use std::env;
use std::error::Error;

use serve::{DefaultLogger, HttpFileServer, Logger};

fn main() -> Result<(), Box<dyn Error>> {
    let log_path = "log.txt".to_string();
    let logger = DefaultLogger::new(log_path)?;

    let content_dir = env::current_dir()?.join("docs");

    // make sure the directory to be served exists
    content_dir.try_exists()?;

    let server = HttpFileServer::new("localhost", 9000, content_dir, logger)?;

    server.run()?;

    Ok(())
}
