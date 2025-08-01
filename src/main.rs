use std::error::Error;

use serve::{HttpFileServer, Logger};

fn main() -> Result<(), Box<dyn Error>> {
    let log_path = "./log.txt".to_string();
    let logger = Logger::new(log_path)?;

    let server = HttpFileServer::new("localhost", 9000, logger)?;

    server.run()?;

    Ok(())
}
