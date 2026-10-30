use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

pub const A_SYNC: u32 = 0x434e_5953; // "SYNC"
pub const A_CNXN: u32 = 0x4e58_4e43; // "CNXN"
pub const A_OPEN: u32 = 0x4e45_504f; // "OPEN"
pub const A_OKAY: u32 = 0x5941_4b4f; // "OKAY"
pub const A_CLSE: u32 = 0x4553_4c43; // "CLSE"
pub const A_WRTE: u32 = 0x4554_5257; // "WRTE"
pub const A_AUTH: u32 = 0x4854_5541; // "AUTH"

pub const A_VERSION: u32 = 0x0100_0000;
pub const MAX_ADB_DATA: u32 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdbPacket {
    pub command: u32,
    pub arg0: u32,
    pub arg1: u32,
    pub payload: Vec<u8>,
}

pub fn adb_payload_checksum(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32))
}

pub fn write_adb_packet<W: Write>(
    writer: &mut W,
    command: u32,
    arg0: u32,
    arg1: u32,
    payload: &[u8],
) -> io::Result<()> {
    let mut header = [0u8; 24];
    let data_len = payload.len() as u32;
    let data_check = adb_payload_checksum(payload);
    let magic = command ^ 0xffff_ffff;

    header[0..4].copy_from_slice(&command.to_le_bytes());
    header[4..8].copy_from_slice(&arg0.to_le_bytes());
    header[8..12].copy_from_slice(&arg1.to_le_bytes());
    header[12..16].copy_from_slice(&data_len.to_le_bytes());
    header[16..20].copy_from_slice(&data_check.to_le_bytes());
    header[20..24].copy_from_slice(&magic.to_le_bytes());

    writer.write_all(&header)?;
    if !payload.is_empty() {
        writer.write_all(payload)?;
    }
    writer.flush()
}

pub fn read_adb_packet<R: Read>(reader: &mut R) -> io::Result<AdbPacket> {
    let mut header = [0u8; 24];
    reader.read_exact(&mut header)?;

    let command = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let arg0 = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let arg1 = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let data_len = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
    let _data_check = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    let magic = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);

    if magic != (command ^ 0xffff_ffff) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid ADB packet magic: cmd=0x{:08x}, magic=0x{:08x}",
                command, magic
            ),
        ));
    }
    if data_len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ADB payload length too large: {}", data_len),
        ));
    }

    let mut payload = vec![0u8; data_len];
    if data_len > 0 {
        reader.read_exact(&mut payload)?;
    }

    Ok(AdbPacket {
        command,
        arg0,
        arg1,
        payload,
    })
}

#[derive(Clone, Debug)]
pub struct AdbClient {
    addr: SocketAddr,
}

impl AdbClient {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub fn from_host_port(host: &str, port: u16) -> Result<Self, String> {
        let target = format!("{}:{}", host, port);
        let addr = target
            .to_socket_addrs()
            .map_err(|e| format!("failed to resolve ADB target '{}': {}", target, e))?
            .next()
            .ok_or_else(|| format!("no socket address found for '{}'", target))?;
        Ok(Self { addr })
    }

    pub fn parse_target(target: &str) -> Result<Self, String> {
        let trimmed = target.trim();
        let with_port = if trimmed.contains(':') {
            trimmed.to_string()
        } else {
            format!("{}:5555", trimmed)
        };
        let addr = with_port
            .to_socket_addrs()
            .map_err(|e| format!("failed to resolve ADB address '{}': {}", with_port, e))?
            .next()
            .ok_or_else(|| format!("no socket address found for '{}'", with_port))?;
        Ok(Self { addr })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn connect_once(&self, io_timeout: Duration) -> Result<TcpStream, String> {
        let connect_timeout = io_timeout.min(Duration::from_millis(2500));
        let mut stream = TcpStream::connect_timeout(&self.addr, connect_timeout)
            .map_err(|e| format!("ADB connect to {} failed: {}", self.addr, e))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(io_timeout));
        let _ = stream.set_write_timeout(Some(io_timeout));

        write_adb_packet(&mut stream, A_CNXN, A_VERSION, MAX_ADB_DATA, b"host::\0")
            .map_err(|e| format!("ADB CNXN write failed: {}", e))?;

        loop {
            let pkt = read_adb_packet(&mut stream)
                .map_err(|e| format!("ADB CNXN read failed: {}", e))?;
            match pkt.command {
                A_CNXN => return Ok(stream),
                A_AUTH => {
                    return Err(
                        "ADB target requested authentication (A_AUTH), expected unauthenticated root adbd".to_string(),
                    );
                }
                _ => continue,
            }
        }
    }

    fn connect_with_retry(&self, total_timeout: Duration) -> Result<TcpStream, String> {
        let deadline = Instant::now() + total_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let attempt_timeout = remaining.clamp(Duration::from_millis(800), Duration::from_secs(4));
            match self.connect_once(attempt_timeout) {
                Ok(s) => return Ok(s),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(e);
                    }
                    thread::sleep(Duration::from_millis(150));
                }
            }
        }
    }

    pub fn wait_until_ready(&self, timeout: Duration) -> Result<(), String> {
        let out = self.shell("echo ADB_READY", timeout)?;
        if out.contains("ADB_READY") {
            Ok(())
        } else {
            Err(format!("unexpected ADB readiness probe output: '{}'", out))
        }
    }

    pub fn shell(&self, command: &str, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        let mut last_err = String::from("ADB shell timed out");

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let io_timeout = remaining.max(Duration::from_millis(1500));
            let mut stream = match self.connect_with_retry(io_timeout) {
                Ok(s) => s,
                Err(e) => {
                    last_err = e;
                    break;
                }
            };
            let _ = stream.set_read_timeout(Some(io_timeout));
            let _ = stream.set_write_timeout(Some(io_timeout));

            let local_id = 1u32;
            let mut open_payload = Vec::with_capacity(7 + command.len());
            open_payload.extend_from_slice(b"shell:");
            open_payload.extend_from_slice(command.as_bytes());
            open_payload.push(0);

            if let Err(e) = write_adb_packet(&mut stream, A_OPEN, local_id, 0, &open_payload) {
                last_err = format!("ADB OPEN failed: {}", e);
                let _ = stream.shutdown(Shutdown::Both);
                thread::sleep(Duration::from_millis(120));
                continue;
            }

            let mut output = Vec::new();
            let mut got_okay_or_data = false;
            let mut stream_err = None;

            loop {
                match read_adb_packet(&mut stream) {
                    Ok(pkt) => match pkt.command {
                        A_OKAY => {
                            got_okay_or_data = true;
                        }
                        A_WRTE => {
                            got_okay_or_data = true;
                            let remote_id = pkt.arg0;
                            output.extend_from_slice(&pkt.payload);
                            if let Err(e) =
                                write_adb_packet(&mut stream, A_OKAY, local_id, remote_id, b"")
                            {
                                stream_err = Some(format!("ADB OKAY write failed: {}", e));
                                break;
                            }
                        }
                        A_CLSE => {
                            let remote_id = pkt.arg0;
                            let _ =
                                write_adb_packet(&mut stream, A_CLSE, local_id, remote_id, b"");
                            break;
                        }
                        _ => {}
                    },
                    Err(e) => {
                        if got_okay_or_data && e.kind() == io::ErrorKind::UnexpectedEof {
                            break;
                        }
                        stream_err = Some(format!("ADB stream read failed: {}", e));
                        break;
                    }
                }
            }

            let _ = stream.shutdown(Shutdown::Both);

            if let Some(err) = stream_err {
                if !got_okay_or_data && Instant::now() < deadline {
                    last_err = err;
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
                if output.is_empty() {
                    return Err(err);
                }
            }

            let text = String::from_utf8_lossy(&output).replace("\r\n", "\n");
            return Ok(text.trim().to_string());
        }

        Err(last_err)
    }

    pub fn pull_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        timeout: Duration,
    ) -> Result<u64, String> {
        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut stream = self.connect_with_retry(timeout)?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));

        let local_id = 1u32;
        write_adb_packet(&mut stream, A_OPEN, local_id, 0, b"sync:\0")
            .map_err(|e| format!("ADB OPEN sync: failed: {}", e))?;

        let remote_id = loop {
            let pkt = read_adb_packet(&mut stream)
                .map_err(|e| format!("ADB sync OPEN response failed: {}", e))?;
            if pkt.command == A_OKAY {
                break pkt.arg0;
            }
            if pkt.command == A_CLSE {
                let _ = stream.shutdown(Shutdown::Both);
                return Err("ADB sync stream closed immediately by remote".to_string());
            }
        };

        let path_bytes = remote_path.as_bytes();
        let mut recv_cmd = Vec::with_capacity(8 + path_bytes.len());
        recv_cmd.extend_from_slice(b"RECV");
        recv_cmd.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        recv_cmd.extend_from_slice(path_bytes);

        write_adb_packet(&mut stream, A_WRTE, local_id, remote_id, &recv_cmd)
            .map_err(|e| format!("ADB sync RECV write failed: {}", e))?;

        let mut out_file = File::create(local_path)
            .map_err(|e| format!("failed to create '{}': {}", local_path.display(), e))?;
        let mut total_written = 0u64;
        let mut sync_buf: Vec<u8> = Vec::new();
        let mut done = false;

        while !done {
            let pkt = read_adb_packet(&mut stream)
                .map_err(|e| format!("ADB sync read failed: {}", e))?;
            match pkt.command {
                A_OKAY => {}
                A_WRTE => {
                    write_adb_packet(&mut stream, A_OKAY, local_id, pkt.arg0, b"")
                        .map_err(|e| format!("ADB sync OKAY failed: {}", e))?;
                    sync_buf.extend_from_slice(&pkt.payload);

                    loop {
                        if sync_buf.len() < 8 {
                            break;
                        }
                        let tag = [sync_buf[0], sync_buf[1], sync_buf[2], sync_buf[3]];
                        let len = u32::from_le_bytes([
                            sync_buf[4],
                            sync_buf[5],
                            sync_buf[6],
                            sync_buf[7],
                        ]) as usize;

                        match &tag {
                            b"DATA" => {
                                if sync_buf.len() < 8 + len {
                                    break;
                                }
                                out_file
                                    .write_all(&sync_buf[8..8 + len])
                                    .map_err(|e| format!("file write failed: {}", e))?;
                                total_written += len as u64;
                                sync_buf.drain(0..8 + len);
                            }
                            b"DONE" => {
                                sync_buf.drain(0..8);
                                done = true;
                                break;
                            }
                            b"FAIL" => {
                                if sync_buf.len() < 8 + len {
                                    break;
                                }
                                let msg =
                                    String::from_utf8_lossy(&sync_buf[8..8 + len]).to_string();
                                let _ = stream.shutdown(Shutdown::Both);
                                return Err(format!("ADB sync RECV failed: {}", msg));
                            }
                            other => {
                                let _ = stream.shutdown(Shutdown::Both);
                                return Err(format!(
                                    "unknown ADB sync tag: {:?}",
                                    String::from_utf8_lossy(other)
                                ));
                            }
                        }
                    }
                }
                A_CLSE => {
                    let _ = write_adb_packet(&mut stream, A_CLSE, local_id, pkt.arg0, b"");
                    break;
                }
                _ => {}
            }
        }

        let _ = write_adb_packet(&mut stream, A_CLSE, local_id, remote_id, b"");
        let _ = stream.shutdown(Shutdown::Both);
        out_file
            .flush()
            .map_err(|e| format!("file flush failed: {}", e))?;
        Ok(total_written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_adb_packet_framing_and_checksum() {
        let payload = b"shell:hostname\0";
        let mut buf = Vec::new();
        write_adb_packet(&mut buf, A_OPEN, 1, 0, payload).unwrap();
        assert_eq!(buf.len(), 24 + payload.len());

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_adb_packet(&mut cursor).unwrap();
        assert_eq!(decoded.command, A_OPEN);
        assert_eq!(decoded.arg0, 1);
        assert_eq!(decoded.arg1, 0);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_adb_client_shell_against_mock_adbd() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let cnxn = read_adb_packet(&mut stream).unwrap();
            assert_eq!(cnxn.command, A_CNXN);
            write_adb_packet(
                &mut stream,
                A_CNXN,
                A_VERSION,
                MAX_ADB_DATA,
                b"device::ro.product.name=yandexmini\0",
            )
            .unwrap();

            let open = read_adb_packet(&mut stream).unwrap();
            assert_eq!(open.command, A_OPEN);
            assert_eq!(open.payload, b"shell:hostname\0");
            let client_id = open.arg0;
            let device_id = 42u32;

            write_adb_packet(&mut stream, A_OKAY, device_id, client_id, b"").unwrap();
            write_adb_packet(
                &mut stream,
                A_WRTE,
                device_id,
                client_id,
                b"qemu-yandexmini\r\n",
            )
            .unwrap();

            let ack = read_adb_packet(&mut stream).unwrap();
            assert_eq!(ack.command, A_OKAY);
            assert_eq!(ack.arg0, client_id);
            assert_eq!(ack.arg1, device_id);

            write_adb_packet(&mut stream, A_CLSE, device_id, client_id, b"").unwrap();
            let _ = read_adb_packet(&mut stream);
        });

        let client = AdbClient::new(addr);
        let out = client
            .shell("hostname", Duration::from_secs(3))
            .expect("shell command should succeed");
        assert_eq!(out, "qemu-yandexmini");
        server.join().unwrap();
    }
}
