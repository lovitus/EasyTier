//! Profiling-bundle-only TCP fixture and load generator.
//!
//! This file is intentionally outside the Cargo workspace. It is compiled directly by the
//! profiling workflow and must never be linked into or packaged with production EasyTier builds.

use std::{
    env,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::ExitCode,
    time::{Duration, Instant},
};

const MAGIC: [u8; 8] = *b"ETPERF01";
const HEADER_LEN: usize = 24;
const RESULT_LEN: usize = 16;
const READY: u8 = 0xa5;
const GO: u8 = 0x5a;
const BUFFER_SIZE: usize = 64 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TRANSFER_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Upload,
    Download,
}

impl Direction {
    fn code(self) -> u8 {
        match self {
            Self::Upload => 1,
            Self::Download => 2,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "upload" => Ok(Self::Upload),
            "download" => Ok(Self::Download),
            _ => Err(format!(
                "invalid direction {value:?}; expected upload or download"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

#[derive(Debug)]
struct TransferResult {
    direction: Direction,
    bytes: u64,
    elapsed_ns: u64,
    client_elapsed_ns: u64,
    server_elapsed_ns: u64,
}

fn usage() -> &'static str {
    "Usage:\n\
  easytier-perf-probe server --listen IP:PORT --sessions N [--timeout-seconds N]\n\
  easytier-perf-probe client --target IP:PORT --direction upload|download --bytes N [--timeout-seconds N]"
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("easytier-perf-probe: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage().to_owned());
    };
    if command == "-h" || command == "--help" {
        println!("{}", usage());
        return Ok(());
    }
    match command {
        "server" => run_server_command(&args[1..]),
        "client" => run_client_command(&args[1..]),
        _ => Err(format!("unknown command {command:?}\n{}", usage())),
    }
}

fn option(args: &[String], name: &str) -> Result<String, String> {
    let mut matches = args.windows(2).filter(|pair| pair[0] == name);
    let value = matches
        .next()
        .map(|pair| pair[1].clone())
        .ok_or_else(|| format!("missing required option {name}"))?;
    if matches.next().is_some() {
        return Err(format!("option {name} was provided more than once"));
    }
    Ok(value)
}

fn parse_u64(value: &str, name: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|error| format!("invalid {name} {value:?}: {error}"))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(format!(
            "{name} must be in {minimum}..={maximum}, got {parsed}"
        ));
    }
    Ok(parsed)
}

fn timeout(args: &[String]) -> Result<Duration, String> {
    let seconds = match args.windows(2).find(|pair| pair[0] == "--timeout-seconds") {
        Some(pair) => parse_u64(&pair[1], "timeout-seconds", 1, 3600)?,
        None => DEFAULT_TIMEOUT_SECS,
    };
    Ok(Duration::from_secs(seconds))
}

fn parse_addr(value: &str, name: &str) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("invalid {name} {value:?}: {error}"))
}

fn run_server_command(args: &[String]) -> Result<(), String> {
    let listen = parse_addr(&option(args, "--listen")?, "listen address")?;
    let sessions = parse_u64(&option(args, "--sessions")?, "sessions", 1, 1024)? as usize;
    let timeout = timeout(args)?;
    let listener = TcpListener::bind(listen).map_err(|error| format!("bind {listen}: {error}"))?;
    serve(listener, sessions, timeout, true).map_err(|error| error.to_string())
}

fn run_client_command(args: &[String]) -> Result<(), String> {
    let target = parse_addr(&option(args, "--target")?, "target address")?;
    let direction = Direction::parse(&option(args, "--direction")?)?;
    let bytes = parse_u64(&option(args, "--bytes")?, "bytes", 1, MAX_TRANSFER_BYTES)?;
    let result = transfer(target, direction, bytes, timeout(args)?).map_err(|error| {
        format!(
            "{} transfer to {target} failed: {error}",
            direction.as_str()
        )
    })?;
    let bits_per_second = result.bytes as f64 * 8_000_000_000.0 / result.elapsed_ns as f64;
    println!(
        "{{\"schema_version\":1,\"ok\":true,\"direction\":\"{}\",\"bytes\":{},\"elapsed_ns\":{},\"client_elapsed_ns\":{},\"server_elapsed_ns\":{},\"bits_per_second\":{:.3}}}",
        result.direction.as_str(),
        result.bytes,
        result.elapsed_ns,
        result.client_elapsed_ns,
        result.server_elapsed_ns,
        bits_per_second,
    );
    Ok(())
}

fn serve(
    listener: TcpListener,
    sessions: usize,
    timeout: Duration,
    announce: bool,
) -> io::Result<()> {
    if announce {
        println!(
            "{{\"schema_version\":1,\"event\":\"ready\",\"listen\":\"{}\",\"sessions\":{sessions}}}",
            listener.local_addr()?
        );
        io::stdout().flush()?;
    }
    for _ in 0..sessions {
        let (mut stream, _) = listener.accept()?;
        configure_stream(&stream, timeout)?;
        handle_server_session(&mut stream)?;
    }
    if announce {
        println!("{{\"schema_version\":1,\"event\":\"complete\",\"sessions\":{sessions}}}");
        io::stdout().flush()?;
    }
    Ok(())
}

fn configure_stream(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(())
}

fn handle_server_session(stream: &mut TcpStream) -> io::Result<()> {
    let mut header = [0u8; HEADER_LEN];
    stream.read_exact(&mut header)?;
    let (direction, requested) = decode_header(&header)?;
    stream.write_all(&[READY])?;
    stream.flush()?;
    let mut go = [0u8; 1];
    stream.read_exact(&mut go)?;
    if go[0] != GO {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid transfer start marker",
        ));
    }

    let started = Instant::now();
    match direction {
        Direction::Upload => read_exact_bytes(stream, requested)?,
        Direction::Download => write_exact_bytes(stream, requested)?,
    }
    let elapsed_ns = duration_ns(started.elapsed());
    let mut result = [0u8; RESULT_LEN];
    result[..8].copy_from_slice(&requested.to_be_bytes());
    result[8..].copy_from_slice(&elapsed_ns.to_be_bytes());
    stream.write_all(&result)?;
    stream.flush()
}

fn transfer(
    target: SocketAddr,
    direction: Direction,
    bytes: u64,
    timeout: Duration,
) -> io::Result<TransferResult> {
    let mut stream = TcpStream::connect_timeout(&target, timeout)?;
    configure_stream(&stream, timeout)?;
    stream.write_all(&encode_header(direction, bytes))?;
    stream.flush()?;
    let mut ready = [0u8; 1];
    stream.read_exact(&mut ready)?;
    if ready[0] != READY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid server ready marker",
        ));
    }
    stream.write_all(&[GO])?;
    stream.flush()?;

    let started = Instant::now();
    match direction {
        Direction::Upload => write_exact_bytes(&mut stream, bytes)?,
        Direction::Download => read_exact_bytes(&mut stream, bytes)?,
    }
    let data_elapsed_ns = duration_ns(started.elapsed());
    let mut encoded_result = [0u8; RESULT_LEN];
    stream.read_exact(&mut encoded_result)?;
    let server_bytes = u64::from_be_bytes(encoded_result[..8].try_into().unwrap());
    let server_elapsed_ns = u64::from_be_bytes(encoded_result[8..].try_into().unwrap());
    if server_bytes != bytes || server_elapsed_ns == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid server result: bytes={server_bytes}, elapsed_ns={server_elapsed_ns}"),
        ));
    }
    let client_elapsed_ns = duration_ns(started.elapsed());
    let elapsed_ns = match direction {
        Direction::Upload => server_elapsed_ns,
        Direction::Download => data_elapsed_ns,
    }
    .max(1);
    Ok(TransferResult {
        direction,
        bytes,
        elapsed_ns,
        client_elapsed_ns,
        server_elapsed_ns,
    })
}

fn encode_header(direction: Direction, bytes: u64) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[..8].copy_from_slice(&MAGIC);
    header[8] = direction.code();
    header[16..].copy_from_slice(&bytes.to_be_bytes());
    header
}

fn decode_header(header: &[u8; HEADER_LEN]) -> io::Result<(Direction, u64)> {
    if header[..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid protocol magic or version",
        ));
    }
    if header[9..16].iter().any(|value| *value != 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-zero reserved header bytes",
        ));
    }
    let direction = match header[8] {
        1 => Direction::Upload,
        2 => Direction::Download,
        value => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid direction code {value}"),
            ));
        }
    };
    let bytes = u64::from_be_bytes(header[16..].try_into().unwrap());
    if bytes == 0 || bytes > MAX_TRANSFER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid transfer length {bytes}"),
        ));
    }
    Ok((direction, bytes))
}

fn read_exact_bytes(stream: &mut TcpStream, bytes: u64) -> io::Result<()> {
    let mut remaining = bytes;
    let mut buffer = [0u8; BUFFER_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = stream.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("transfer ended with {remaining} bytes remaining"),
            ));
        }
        remaining -= read as u64;
    }
    Ok(())
}

fn write_exact_bytes(stream: &mut TcpStream, bytes: u64) -> io::Result<()> {
    let mut remaining = bytes;
    let buffer = [0xa5u8; BUFFER_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        stream.write_all(&buffer[..wanted])?;
        remaining -= wanted as u64;
    }
    stream.flush()
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn header_round_trip_and_validation() {
        for direction in [Direction::Upload, Direction::Download] {
            let encoded = encode_header(direction, 123_456);
            assert_eq!(decode_header(&encoded).unwrap(), (direction, 123_456));
        }
        let mut invalid = encode_header(Direction::Upload, 1);
        invalid[9] = 1;
        assert_eq!(
            decode_header(&invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn upload_and_download_are_byte_exact() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server =
            thread::spawn(move || serve(listener, 2, Duration::from_secs(10), false).unwrap());
        for direction in [Direction::Upload, Direction::Download] {
            let result = transfer(
                address,
                direction,
                2 * 1024 * 1024 + 17,
                Duration::from_secs(10),
            )
            .unwrap();
            assert_eq!(result.direction, direction);
            assert_eq!(result.bytes, 2 * 1024 * 1024 + 17);
            assert!(result.elapsed_ns > 0);
        }
        server.join().unwrap();
    }
}
