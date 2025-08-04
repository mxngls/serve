use std::env;
use std::error::Error;

use serve::{HttpFileServer, Logger};

fn main() -> Result<(), Box<dyn Error>> {
    let log_path = "./log.txt".to_string();
    let logger = Logger::new(log_path)?;

    let exec_dir = env::current_dir()?;
    let root_dir = exec_dir.parent().unwrap();
    let content_dir = root_dir.join("docs");

    // make sure the directory to be served exists
    content_dir.try_exists()?;

    let server = HttpFileServer::new("localhost", 9000, content_dir, logger)?;

    server.run()?;

    Ok(())
}
