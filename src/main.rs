use std::{
    error::Error,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    result::Result,
    string::String,
    vec::Vec,
};

fn parse_request(request: &[u8]) -> Result<Vec<String>, Box<dyn Error>> {
    let lines: Vec<String> = str::from_utf8(request)?
        .split("\r\n")
        .map(|s| s.to_string())
        .collect();

    let Some(request_line) = lines.first() else {
        return Err("Empy request line".into());
    };

    let parts: Vec<&str> = request_line.split(" ").collect();
    if parts.len() != 3 {
        return Err("Invalid request line format".into());
    }
    let (method, uri, version) = (parts[0], parts[1], parts[2]);

    println!("Method: {}", method);
    println!("URI: {}", uri);
    println!("version: {}", version);

    Ok(lines)
}

fn handle_client(mut stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(0) => return Ok(()), // Connection closed
        Ok(n) => {
            parse_request(&buffer[..n])?.iter();
            // .for_each(|line| println!("{}", line));
        }
        Err(e) => return Err(Box::new(e)),
    }

    let response = "HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nHey it's me!";
    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:80")?;

    for stream in listener.incoming() {
        let stream = stream?;

        handle_client(stream)?;
    }

    Ok(())
}
